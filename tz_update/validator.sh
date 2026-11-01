#!/bin/bash
#
# verify_timezone_network.sh
#
# Network-wide, read-only timezone validator for MUX / EDS systems.
#
# Usage:
#   ./verify_timezone_network.sh [--all|--eds|--db]
#   ./verify_timezone_network.sh --all --site YVR
#
# Environment overrides:
#   SSH_USER=geeds
#   SSH_TIMEOUT=5
#   CONNECTION_MODE=0|1|2
#
# Requirements on controller:
#   bash, ssh, sqlite3, psql (only if querying local PostgreSQL),
#   ip, awk, sed, grep, sort, date
#
# Remote hosts require:
#   bash, rpm, date, readlink, zdump (required for transition-rule classification).
# The remote checks are read-only and run as SSH_USER; sudo is not required.
#
# The script does not change timezone, clock, packages, or PostgreSQL config.
#

set -u
umask 077

RAIN1="10.3.3.1"
RAIN2="10.3.3.2"
STANDALONE="10.3.40.1"

DB_NAME="mux"
DB_USER="postgres"
DB_PORT="5432"
DB_PASSWORD="postgres"

EDS_CONFIG_DB="/var/eds/edsconfigs.db"
SSH_USER="${SSH_USER:-geeds}"
SSH_TIMEOUT="${SSH_TIMEOUT:-5}"

SCOPE="all"
SITE_FILTER=""
REVIEW_ONLY=0
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
STAMP="$(date +%Y%m%d_%H%M%S)"
OUT_DIR="${SCRIPT_DIR}/timezone_validation_${STAMP}"
LOG_FILE="${OUT_DIR}/timezone_validation.log"
CSV_FILE="${OUT_DIR}/timezone_validation.csv"
SUMMARY_FILE="${OUT_DIR}/timezone_validation_summary.txt"

PASS_COUNT=0
FAIL_COUNT=0
WARN_COUNT=0
OFFLINE_COUNT=0
TOTAL_COUNT=0
TIMEZONE_PASS_COUNT=0
TIMEZONE_FAIL_COUNT=0
CLOCK_PASS_COUNT=0
CLOCK_FAIL_COUNT=0

usage() {
    cat <<EOF
Usage: $0 [--all|--eds|--db] [--site SITE_ID] [--review]

  --all       Validate inventory hosts and RAIN database hosts (default)
  --eds       Validate EDS hosts (host.Kxxx / host.Xxxx)
  --db        Validate RAIN1 and RAIN2 database hosts
  --site ID   Only validate inventory for the specified site ID
  --review    Review each machine's configured timezone rules without enforcing the site timezone
  -h, --help  Show this help

Examples:
  $0 --all
  $0 --eds
  $0 --db
  $0 --all --site YVR

Environment:
  SSH_USER=geeds
  SSH_TIMEOUT=5
  CONNECTION_MODE=0|1|2
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --all) SCOPE="all"; shift ;;
        --eds) SCOPE="eds"; shift ;;
        --db) SCOPE="db"; shift ;;
        --review) REVIEW_ONLY=1; shift ;;
        --site)
            [ "$#" -ge 2 ] || { echo "Missing value for --site" >&2; exit 2; }
            SITE_FILTER="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; usage; exit 2 ;;
    esac
done

mkdir -p "$OUT_DIR" || exit 1
exec > >(tee -a "$LOG_FILE") 2>&1

log() { printf '[%s] %s\n' "$(date '+%F %T')" "$*"; }
trim() { sed 's/^[[:space:]]*//; s/[[:space:]]*$//'; }

csv_escape() {
    local s=${1-}
    s=${s//$'\\n'/ }
    s=${s//$'\\r'/ }
    s=${s//\"/\"\"}
    printf '"%s"' "$s"
}

write_csv_row() {
    local first=1 value
    for value in "$@"; do
        [ "$first" -eq 1 ] || printf ',' >> "$CSV_FILE"
        csv_escape "$value" >> "$CSV_FILE"
        first=0
    done
    printf '\n' >> "$CSV_FILE"
}

site_to_timezone() {
    case "$(printf '%s' "$1" | tr '[:lower:]' '[:upper:]')" in
        YVR|YLW|YXS|YXX|YYJ) echo "America/Vancouver" ;;
        YEG|YMM|YYC) echo "America/Edmonton" ;;
        YQR|YXE) echo "America/Regina" ;;
        YWG) echo "America/Winnipeg" ;;
        YZF) echo "America/Yellowknife" ;;
        YXY) echo "America/Whitehorse" ;;
        YFB) echo "America/Iqaluit" ;;
        YHM|YHU|YKF|KMT|YOW|YQB|YQG|YSB|YTZ|YUL|YXU|YYZ)
            echo "America/Toronto" ;;
        YQT) echo "America/Thunder_Bay" ;;
        YFC|YQM|YSJ|YYG) echo "America/Moncton" ;;
        YHZ) echo "America/Halifax" ;;
        YQX) echo "America/St_Johns" ;;
        *) return 1 ;;
    esac
}

get_connection_mode() {
    local mode="${CONNECTION_MODE:-}"
    if [[ "$mode" =~ ^[012]$ ]]; then
        printf '%s' "$mode"
        return
    fi

    if command -v sqlite3 >/dev/null 2>&1 && [ -r "$EDS_CONFIG_DB" ]; then
        # The current EDS configuration stores the mode in config_id_str.
        # Accept only an unambiguous 0/1/2 value.
        mode=$(sqlite3 "$EDS_CONFIG_DB" \
            "SELECT trim(config_id_str) FROM eds_configs
             WHERE trim(config_id_str) IN ('0','1','2')
             LIMIT 1;" 2>/dev/null | head -n 1)
        if [[ "$mode" =~ ^[012]$ ]]; then
            printf '%s' "$mode"
            return
        fi
    fi

    # Safe discovery fallback: try both RAIN databases before standalone.
    printf '1'
}

candidate_order() {
    case "$1" in
        0) printf '%s\n' "$STANDALONE" "$RAIN1" "$RAIN2" ;;
        1|2|*) printf '%s\n' "$RAIN1" "$RAIN2" "$STANDALONE" ;;
    esac
}

local_ipv4_list() {
    {
        hostname -I 2>/dev/null | tr ' ' '\n'
        ip -o -4 addr show 2>/dev/null | awk '{sub(/\/.*/, "", $4); print $4}'
        printf '127.0.0.1\n'
    } | sed '/^$/d' | sort -u
}

IS_LOCAL=0
is_local_ip() {
    local target="$1"
    if local_ipv4_list | grep -Fxq "$target"; then
        return 0
    fi
    return 1
}

ssh_opts=(
    -o BatchMode=yes
    -o ConnectTimeout="$SSH_TIMEOUT"
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile=/dev/null
    -o LogLevel=ERROR
)

run_remote_sql() {
    local ip="$1"
    local sql="$2"
    if is_local_ip "$ip"; then
        PGPASSWORD="$DB_PASSWORD" psql -X -w \
            -h 127.0.0.1 -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" \
            -At -F $'\t' -c "$sql" 2>&1
    else
        ssh "${ssh_opts[@]}" "${SSH_USER}@${ip}" \
            "PGPASSWORD='${DB_PASSWORD}' psql -X -w -h 127.0.0.1 -p ${DB_PORT} -U ${DB_USER} -d ${DB_NAME} -At -F '\t' -c \"${sql//\"/\\\"}\"" 2>&1
    fi
}

discover_inventory() {
    local mode candidate result site_result inventory_result
    local site_query="SELECT DISTINCT trim(site_id) FROM public.site_admin WHERE site_id IS NOT NULL AND trim(site_id) <> '' ORDER BY 1;"
    local host_query="SELECT host_name || E'\t' || host(host_ip) FROM public.host_admin WHERE host_name IS NOT NULL AND trim(host_name) <> '' AND host_ip IS NOT NULL AND host_ip <> '127.0.0.1'::inet ORDER BY host_ip;"

    mode=$(get_connection_mode)
    log "Detected connection_mode: $mode"

    while IFS= read -r candidate; do
        [ -n "$candidate" ] || continue
        log "Trying inventory database at $candidate"

        result=$(run_remote_sql "$candidate" "$site_query")
        if [ "$?" -ne 0 ] || [ -z "$result" ]; then
            log "No usable site inventory from $candidate: $result"
            continue
        fi

        site_result=$(printf '%s\n' "$result" | grep -v '^$' | trim)
        if [ -n "$SITE_FILTER" ]; then
            if ! printf '%s\n' "$site_result" | grep -Fxq "$SITE_FILTER"; then
                log "Requested site $SITE_FILTER not found on $candidate"
                continue
            fi
            site_result="$SITE_FILTER"
        fi

        # The inventory schema currently provides one site_id per site database.
        # Multiple distinct IDs are treated as ambiguous rather than guessed.
        if [ "$(printf '%s\n' "$site_result" | grep -c .)" -ne 1 ]; then
            log "Ambiguous site_admin result on $candidate:"
            printf '%s\n' "$site_result"
            continue
        fi

        SITE_ID="$site_result"
        inventory_result=$(run_remote_sql "$candidate" "$host_query")
        if [ "$?" -ne 0 ]; then
            log "Unable to read host_admin from $candidate: $inventory_result"
            continue
        fi

        INVENTORY_DB_IP="$candidate"
        INVENTORY_DATA="$inventory_result"
        log "Using inventory database $candidate; site_id=$SITE_ID"
        return 0
    done < <(candidate_order "$mode")

    return 1
}

# This validator runs on the target host as SSH_USER. It performs read-only
# checks and emits key=value records; none of its commands require root.
read -r -d '' REMOTE_VALIDATOR <<'REMOTE_EOF' || true
#!/bin/bash
set +e
EXPECTED_TZ="${1:-}"
HOST_LABEL="${2:-unknown}"
REVIEW_ONLY="${3:-0}"
CLOCK_EXPECTED_TZ="${4:-$EXPECTED_TZ}"

emit() { printf '%s=%s\n' "$1" "${2//$'\n'/ }"; }

# Resolve zdump explicitly. Non-interactive SSH sessions on Fedora 20 /
# CentOS 7 may omit /usr/sbin from PATH even when zdump is installed.
ZDUMP_BIN=""
for candidate in /usr/sbin/zdump /sbin/zdump /usr/bin/zdump /bin/zdump; do
    if [ -x "$candidate" ]; then
        ZDUMP_BIN="$candidate"
        break
    fi
done

failures=0
warnings=0
timezone_failures=0
clock_failures=0

# Identify configured timezone through the localtime symlink.
LOCALTIME_LINK="no"
LOCALTIME_TARGET=""
DETECTED_TZ="UNKNOWN"

if [ -L /etc/localtime ]; then
    LOCALTIME_LINK="yes"
    LOCALTIME_TARGET=$(readlink -f /etc/localtime 2>/dev/null)
    case "$LOCALTIME_TARGET" in
        /usr/share/zoneinfo/*)
            DETECTED_TZ="${LOCALTIME_TARGET#/usr/share/zoneinfo/}"
            ;;
    esac
fi

if [ "$LOCALTIME_LINK" != "yes" ]; then
    failures=$((failures+1)); timezone_failures=$((timezone_failures+1))
fi
if [ -z "$LOCALTIME_TARGET" ] || [ ! -f "$LOCALTIME_TARGET" ]; then
    failures=$((failures+1)); timezone_failures=$((timezone_failures+1))
fi
if [ "$DETECTED_TZ" = "UNKNOWN" ]; then
    failures=$((failures+1)); timezone_failures=$((timezone_failures+1))
fi

SYSTEM_MATCH="UNKNOWN"
if [ "$REVIEW_ONLY" = "1" ]; then
    # Review mode deliberately inspects the configured timezone, even when
    # it differs from the timezone mapped from the site's inventory.
    SYSTEM_MATCH="NOT_ENFORCED"
elif [ -n "$EXPECTED_TZ" ] && [ "$DETECTED_TZ" = "$EXPECTED_TZ" ]; then
    SYSTEM_MATCH="PASS"
elif [ -n "$EXPECTED_TZ" ]; then
    SYSTEM_MATCH="FAIL"
    failures=$((failures+1)); timezone_failures=$((timezone_failures+1))
fi

VALIDATION_TZ="$EXPECTED_TZ"
if [ "$REVIEW_ONLY" = "1" ]; then
    VALIDATION_TZ="$DETECTED_TZ"
fi

LOCAL_TIME=$(date '+%Y-%m-%d %H:%M:%S %Z %z' 2>/dev/null)
UTC_TIME=$(date -u '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null)

TZDATA=$(rpm -q tzdata 2>/dev/null)
if [ $? -ne 0 ]; then TZDATA="NOT_INSTALLED"; failures=$((failures+1)); fi

TZDATA_JAVA=$(rpm -q tzdata-java 2>/dev/null)
if [ $? -ne 0 ]; then TZDATA_JAVA="NOT_INSTALLED (OPTIONAL)"; fi

# PostgreSQL detection:
# Fedora 20:  postgresql.service
# CentOS 7:   postgresql-10.service
#
# Check service state first, then fall back to process detection.
PG_STATUS="NOT_RUNNING"
PG_SERVICE="NONE"
PG_TIMEZONE="N/A"
PG_LOG_TIMEZONE="N/A"
PG_MATCH="N/A"
PG_ACTIVE=0

if command -v systemctl >/dev/null 2>&1; then
    if systemctl is-active --quiet postgresql.service 2>/dev/null; then
        PG_ACTIVE=1
        PG_SERVICE="postgresql.service"
    elif systemctl is-active --quiet postgresql-10.service 2>/dev/null; then
        PG_ACTIVE=1
        PG_SERVICE="postgresql-10.service"
    fi
fi

if [ "$PG_ACTIVE" -eq 0 ] && command -v service >/dev/null 2>&1; then
    if service postgresql status >/dev/null 2>&1; then
        PG_ACTIVE=1
        PG_SERVICE="postgresql"
    elif service postgresql-10 status >/dev/null 2>&1; then
        PG_ACTIVE=1
        PG_SERVICE="postgresql-10"
    fi
fi

# Process fallback for nonstandard service names.
if [ "$PG_ACTIVE" -eq 0 ]; then
    if pgrep -x postgres >/dev/null 2>&1 ||
       pgrep -x postmaster >/dev/null 2>&1 ||
       pgrep -f '[p]ostgres: checkpointer' >/dev/null 2>&1; then
        PG_ACTIVE=1
        PG_SERVICE="PROCESS_DETECTED"
    fi
fi

if [ "$PG_ACTIVE" -eq 1 ]; then
    if command -v psql >/dev/null 2>&1; then
        # Connect over TCP as the invoking account using PostgreSQL password
        # authentication. Do not sudo to the postgres OS account: doing so can
        # emit a harmless cwd permission warning into psql output and corrupt
        # the two-value result. The query returns exactly one pipe-delimited row.
        PG_RESULT=$(PGPASSWORD=postgres psql -X -w \
            -h 127.0.0.1 -p 5432 \
            -U postgres -d mux -At -F '|' \
            -c "SELECT current_setting('TimeZone'), current_setting('log_timezone');" 2>/dev/null)
        PG_RC=$?
        if [ "$PG_RC" -eq 0 ]; then
            PG_STATUS="RUNNING"
            IFS='|' read -r PG_TIMEZONE PG_LOG_TIMEZONE <<< "$PG_RESULT"
            if [ -n "$VALIDATION_TZ" ] &&
               [ "$PG_TIMEZONE" = "$VALIDATION_TZ" ] &&
               [ "$PG_LOG_TIMEZONE" = "$VALIDATION_TZ" ]; then
                PG_MATCH="PASS"
            elif [ -n "$VALIDATION_TZ" ]; then
                PG_MATCH="FAIL"
                failures=$((failures+1)); timezone_failures=$((timezone_failures+1))
            else
                PG_MATCH="CHECK"
            fi
        else
            PG_STATUS="QUERY_FAILED"
            PG_MATCH="WARNING"
            warnings=$((warnings+1))
        fi
    else
        PG_STATUS="PSQL_NOT_INSTALLED"
        PG_MATCH="WARNING"
        warnings=$((warnings+1))
    fi
fi

# ------------------------------------------------------------
# Spring and fall clock-boundary tests.
# zdump is used to locate actual rule transitions, including abbreviation-only
# transitions where the UTC offset does not change (for example MDT -> CST).
# If no transition exists in a month, test a normal 02:00 local boundary.
# These are read-only future/historical rule calculations; the host clock is
# never changed.
# ------------------------------------------------------------
ZDUMP_STATUS="NOT_CHECKED"
DST_POLICY="UNKNOWN"
SPRING_2026_CLOCK_BEHAVIOR="N/A"
FALL_2026_CLOCK_BEHAVIOR="N/A"
SPRING_2027_CLOCK_BEHAVIOR="N/A"
FALL_2027_CLOCK_BEHAVIOR="N/A"
CLOCK_MATCH="N/A"

zone_month_test() {
    local zone="$1" year="$2" month="$3" fallback_date="$4"
    local zdump_output transition_stamp transition_epoch
    local before after before_offset after_offset before_local after_local
    local offset_delta behavior

    zdump_output=$("$ZDUMP_BIN" -v -c "$year,$((year + 1))" "$zone" 2>&1) || return 1

    # zdump emits two records around each transition. Compare UTC offset,
    # DST flag and abbreviation so same-offset abbreviation changes are found.
    transition_stamp=$(printf '%s\n' "$zdump_output" | awk -v wanted_month="$month" '
        /isdst=/ && /gmtoff=/ && $8 == "=" {
            offset=$NF; sub(/^gmtoff=/, "", offset)
            isdst=$(NF-1); sub(/^isdst=/, "", isdst)
            abbr=$(NF-2)
            if (prev_offset != "" && $3 == wanted_month &&
                prev_month == $3 && prev_day == $4 && prev_year == $6 &&
                (offset != prev_offset || isdst != prev_isdst || abbr != prev_abbr)) {
                print $2 " " $3 " " $4 " " $5 " " $6
                exit
            }
            prev_offset=offset
            prev_isdst=isdst
            prev_abbr=abbr
            prev_month=$3
            prev_day=$4
            prev_year=$6
        }')

    if [ -n "$transition_stamp" ]; then
        transition_epoch=$(date -u -d "$transition_stamp UTC" '+%s' 2>/dev/null) || return 1
    else
        # No transition in this month: use an ordinary local 02:00 boundary.
        transition_epoch=$(TZ="$zone" date -d "$year-$fallback_date 02:00:00" '+%s' 2>/dev/null) || return 1
    fi

    before=$((transition_epoch - 1))
    after=$transition_epoch
    before_local=$(TZ="$zone" date -d "@$before" '+%F %T %z %Z' 2>/dev/null) || return 1
    after_local=$(TZ="$zone" date -d "@$after" '+%T %z %Z' 2>/dev/null) || return 1
    before_offset=$(TZ="$zone" date -d "@$before" '+%z' 2>/dev/null) || return 1
    after_offset=$(TZ="$zone" date -d "@$after" '+%z' 2>/dev/null) || return 1

    offset_delta=$(awk -v a="$before_offset" -v b="$after_offset" 'BEGIN {
        av=(substr(a,2,2)*60)+substr(a,4,2); if (substr(a,1,1)=="-") av=-av;
        bv=(substr(b,2,2)*60)+substr(b,4,2); if (substr(b,1,1)=="-") bv=-bv;
        print bv-av;
    }')

    if [ "$offset_delta" -gt 0 ]; then
        behavior="SPRING FORWARD (+${offset_delta} minutes)"
    elif [ "$offset_delta" -lt 0 ]; then
        behavior="CLOCK MOVES BACKWARD (-$((-offset_delta)) minutes)"
    else
        behavior="NO OFFSET CHANGE; CLOCK CONTINUES FORWARD"
    fi

    printf '%s|%s|%s|%s' "$behavior" "$before_local" "$after_local" "$offset_delta"
}

review_timezone_rules() {
    local actual_zone="$1" expected_zone="$2"
    local year spring_test fall_test
    local spring_behavior spring_before spring_after spring_delta
    local fall_behavior fall_before fall_after fall_delta
    local policy_2026 policy_2027

    ZDUMP_STATUS="NOT_CHECKED"
    DST_POLICY="UNKNOWN"
    SPRING_2026_CLOCK_BEHAVIOR="N/A"
    FALL_2026_CLOCK_BEHAVIOR="N/A"
    SPRING_2027_CLOCK_BEHAVIOR="N/A"
    FALL_2027_CLOCK_BEHAVIOR="N/A"
    CLOCK_MATCH="N/A"

    if [ -z "$ZDUMP_BIN" ]; then
        ZDUMP_STATUS="ZDUMP_UNAVAILABLE"
        warnings=$((warnings+1))
        return
    fi

    if [ -z "$actual_zone" ] || [ "$actual_zone" = "UNKNOWN" ] ||
       [ ! -f "/usr/share/zoneinfo/$actual_zone" ]; then
        ZDUMP_STATUS="ACTUAL_ZONE_UNAVAILABLE"
        failures=$((failures+1)); clock_failures=$((clock_failures+1))
        return
    fi

    # Check both seasonal boundaries in 2026 and 2027. zdump evaluates the
    # installed timezone rules for both years, regardless of today's date.
    for year in 2026 2027; do
        spring_test=$(zone_month_test "$actual_zone" "$year" "Mar" "03-15")
        if [ "$?" -ne 0 ] || [ -z "$spring_test" ]; then
            ZDUMP_STATUS="SPRING_${year}_ZDUMP_FAILED"
            CLOCK_MATCH="UNKNOWN"
            failures=$((failures+1)); clock_failures=$((clock_failures+1))
            return
        fi

        fall_test=$(zone_month_test "$actual_zone" "$year" "Nov" "11-01")
        if [ "$?" -ne 0 ] || [ -z "$fall_test" ]; then
            ZDUMP_STATUS="FALL_${year}_ZDUMP_FAILED"
            CLOCK_MATCH="UNKNOWN"
            failures=$((failures+1)); clock_failures=$((clock_failures+1))
            return
        fi

        IFS='|' read -r spring_behavior spring_before spring_after spring_delta <<< "$spring_test"
        IFS='|' read -r fall_behavior fall_before fall_after fall_delta <<< "$fall_test"

        if [ "$year" = "2026" ]; then
            SPRING_2026_CLOCK_BEHAVIOR="${year}-03: ${spring_before} -> ${spring_after}; ${spring_behavior}"
            FALL_2026_CLOCK_BEHAVIOR="${year}-11: ${fall_before} -> ${fall_after}; ${fall_behavior}"
            delta_2026_spring="$spring_delta"
            delta_2026_fall="$fall_delta"
        else
            SPRING_2027_CLOCK_BEHAVIOR="${year}-03: ${spring_before} -> ${spring_after}; ${spring_behavior}"
            FALL_2027_CLOCK_BEHAVIOR="${year}-11: ${fall_before} -> ${fall_after}; ${fall_behavior}"
            delta_2027_spring="$spring_delta"
            delta_2027_fall="$fall_delta"
        fi
    done

    if [ "$REVIEW_ONLY" = "1" ]; then
        DST_POLICY="REVIEW_ONLY"
        CLOCK_MATCH="NOT_ENFORCED"
        ZDUMP_STATUS="REVIEW_ONLY"
        return
    fi

    # Each year may follow one of the supported patterns:
    # seasonal DST (+60/-60), a transition year (+60/0), or permanent/fixed
    # time (0/0). The two years are evaluated independently.
    case "${delta_2026_spring}/${delta_2026_fall}" in
        60/-60) policy_2026="STANDARD_DST" ;;
        60/0)   policy_2026="PERMANENT_DST_TRANSITION_YEAR" ;;
        0/0)    policy_2026="PERMANENT_DST_OR_FIXED_OFFSET" ;;
        *)      policy_2026="UNEXPECTED_TRANSITIONS" ;;
    esac
    case "${delta_2027_spring}/${delta_2027_fall}" in
        60/-60) policy_2027="STANDARD_DST" ;;
        60/0)   policy_2027="PERMANENT_DST_TRANSITION_YEAR" ;;
        0/0)    policy_2027="PERMANENT_DST_OR_FIXED_OFFSET" ;;
        *)      policy_2027="UNEXPECTED_TRANSITIONS" ;;
    esac

    DST_POLICY="2026=${policy_2026};2027=${policy_2027}"
    if [ "$policy_2026" != "UNEXPECTED_TRANSITIONS" ] &&
       [ "$policy_2027" != "UNEXPECTED_TRANSITIONS" ]; then
        CLOCK_MATCH="PASS"
        ZDUMP_STATUS="PASS"
    else
        CLOCK_MATCH="FAIL"
        ZDUMP_STATUS="UNEXPECTED_CLOCK_TRANSITIONS"
        failures=$((failures+1)); clock_failures=$((clock_failures+1))
    fi
}

# Validate spring and fall clock behavior using the actual configured timezone.
# Site timezone identity is validated independently above.
review_timezone_rules "$DETECTED_TZ" "$CLOCK_EXPECTED_TZ"

if [ "$REVIEW_ONLY" = "1" ]; then
    TIMEZONE_RESULT="NOT_ENFORCED"
elif [ "$timezone_failures" -gt 0 ]; then
    TIMEZONE_RESULT="FAIL"
else
    TIMEZONE_RESULT="PASS"
fi

if [ "$REVIEW_ONLY" = "1" ]; then
    CLOCK_MATCH="NOT_ENFORCED"
    CLOCK_RESULT="NOT_ENFORCED"
elif [ "$clock_failures" -gt 0 ]; then
    CLOCK_RESULT="FAIL"
elif [ "$CLOCK_MATCH" = "PASS" ]; then
    CLOCK_RESULT="PASS"
else
    CLOCK_RESULT="WARNING"
fi

OVERALL="PASS"
if [ "$failures" -gt 0 ]; then
    OVERALL="FAIL"
elif [ "$warnings" -gt 0 ]; then
    OVERALL="WARNING"
fi

emit HOST "$HOST_LABEL"
emit EXPECTED_TZ "$EXPECTED_TZ"
emit DETECTED_TZ "$DETECTED_TZ"
emit LOCALTIME_LINK "$LOCALTIME_LINK"
emit LOCALTIME_TARGET "$LOCALTIME_TARGET"
emit LOCAL_TIME "$LOCAL_TIME"
emit UTC_TIME "$UTC_TIME"
emit SYSTEM_MATCH "$SYSTEM_MATCH"
emit TIMEZONE_RESULT "$TIMEZONE_RESULT"
emit TZDATA "$TZDATA"
emit TZDATA_JAVA "$TZDATA_JAVA"
emit PG_STATUS "$PG_STATUS"
emit PG_SERVICE "$PG_SERVICE"
emit PG_TIMEZONE "$PG_TIMEZONE"
emit PG_LOG_TIMEZONE "$PG_LOG_TIMEZONE"
emit PG_MATCH "$PG_MATCH"
emit DST_POLICY "$DST_POLICY"
emit SPRING_2026_CLOCK_BEHAVIOR "$SPRING_2026_CLOCK_BEHAVIOR"
emit FALL_2026_CLOCK_BEHAVIOR "$FALL_2026_CLOCK_BEHAVIOR"
emit SPRING_2027_CLOCK_BEHAVIOR "$SPRING_2027_CLOCK_BEHAVIOR"
emit FALL_2027_CLOCK_BEHAVIOR "$FALL_2027_CLOCK_BEHAVIOR"
emit CLOCK_MATCH "$CLOCK_MATCH"
emit CLOCK_RESULT "$CLOCK_RESULT"
emit ZDUMP_STATUS "$ZDUMP_STATUS"
emit REVIEW_ONLY "$REVIEW_ONLY"
emit FAILURES "$failures"
emit WARNINGS "$warnings"
emit TIMEZONE_FAILURES "$timezone_failures"
emit CLOCK_FAILURES "$clock_failures"
emit OVERALL "$OVERALL"
REMOTE_EOF

run_validator() {
    local host="$1" ip="$2" tz="$3" clock_tz="$4"
    local output rc
    if is_local_ip "$ip"; then
        output=$(printf '%s\n' "$REMOTE_VALIDATOR" | bash -s -- "$tz" "$host" "$REVIEW_ONLY" "$clock_tz" 2>&1)
        rc=$?
    else
        output=$(printf '%s\n' "$REMOTE_VALIDATOR" |
            ssh "${ssh_opts[@]}" "${SSH_USER}@${ip}" \
            "bash -s -- '$tz' '$host' '$REVIEW_ONLY' '$clock_tz'" 2>&1)
        rc=$?
    fi
    printf '%s' "$output"
    return "$rc"
}

# --------------------------- Main ---------------------------

log "Starting network timezone validation"
log "Scope=$SCOPE Site filter=${SITE_FILTER:-none}"
log "Output directory: $OUT_DIR"
log "Clock comparison: March spring-forward and November fall-back boundary tests; actual local times, UTC offsets and abbreviations"

printf '%s\n' \
'"host","ip","site_id","expected_timezone","detected_timezone","localtime_symlink","tzdata","tzdata_java","postgres_status","postgres_service","postgres_timezone","postgres_log_timezone","system_match","postgres_match","timezone_result","dst_policy","spring_2026_clock_behavior","fall_2026_clock_behavior","spring_2027_clock_behavior","fall_2027_clock_behavior","clock_match","clock_result","zdump_status","review_only","overall","details"' \
> "$CSV_FILE"

MODE=$(get_connection_mode)
if ! discover_inventory; then
    log "ERROR: Unable to discover inventory from RAIN1, RAIN2 or standalone."
    log "Check SSH access, PostgreSQL availability, database credentials and schema."
    exit 1
fi

EXPECTED_TZ=$(site_to_timezone "$SITE_ID" 2>/dev/null || true)
CLOCK_EXPECTED_TZ="$EXPECTED_TZ"
if [ "$REVIEW_ONLY" -eq 0 ] && [ -z "$EXPECTED_TZ" ]; then
    log "ERROR: No timezone mapping exists for site_id=$SITE_ID."
    log "Add this site code to site_to_timezone() before validating."
    exit 1
fi
if [ "$REVIEW_ONLY" -eq 1 ]; then
    log "Review-only mode: site timezone enforcement is disabled; each host's configured timezone will be reviewed."
    EXPECTED_TZ="NOT_ENFORCED"
else
    log "Site $SITE_ID maps to $EXPECTED_TZ"
fi

declare -A HOST_IPS
declare -A HOST_NAMES

while IFS=$'\t' read -r host ip; do
    [ -n "${host:-}" ] || continue
    [ -n "${ip:-}" ] || continue
    HOST_IPS["$host"]="$ip"
    HOST_NAMES["$ip"]="$host"
done <<< "$INVENTORY_DATA"

# RAIN database servers are included as named validation targets. The
# standalone address is only a possible inventory-database connection endpoint;
# it is not a host_admin inventory entry and must not be added as a target.
if [ "$SCOPE" != "eds" ]; then
    HOST_IPS["rain1"]="$RAIN1"
    HOST_IPS["rain2"]="$RAIN2"
fi

# Select targets by scope.
declare -a TARGET_NAMES=()
for host in "${!HOST_IPS[@]}"; do
    case "$SCOPE" in
        eds)
            [[ "$host" =~ ^host\.[KXkx][0-9]+$ ]] && TARGET_NAMES+=("$host")
            ;;
        db)
            case "$host" in rain1|rain2) TARGET_NAMES+=("$host");; esac
            ;;
        all)
            TARGET_NAMES+=("$host")
            ;;
    esac
done

if [ "${#TARGET_NAMES[@]}" -eq 0 ]; then
    log "No targets found for scope=$SCOPE"
    exit 1
fi

# Stable ordering by hostname.
mapfile -t TARGET_NAMES < <(printf '%s\n' "${TARGET_NAMES[@]}" | sort -u)

# Avoid validating the same physical host twice if it appears under both
# an inventory hostname and a database-role alias.
declare -A SEEN_TARGET_IPS
log "Discovered ${#TARGET_NAMES[@]} candidate target(s)"
for host in "${TARGET_NAMES[@]}"; do
    ip="${HOST_IPS[$host]}"
    if [ -n "${SEEN_TARGET_IPS[$ip]:-}" ]; then
        log "Skipping duplicate IP $ip ($host); already represented by ${SEEN_TARGET_IPS[$ip]}"
        continue
    fi
    SEEN_TARGET_IPS["$ip"]="$host"
    TOTAL_COUNT=$((TOTAL_COUNT+1))
    log "------------------------------------------------------------"
    log "Validating $host ($ip), expected timezone=$EXPECTED_TZ"

    if is_local_ip "$ip"; then
        log "Target is the controller itself; running validator locally."
    fi

    RESULT=$(run_validator "$host" "$ip" "$EXPECTED_TZ" "$CLOCK_EXPECTED_TZ")
    RC=$?

    if [ "$RC" -ne 0 ]; then
        log "OFFLINE/SSH ERROR: $host ($ip)"
        log "$RESULT"
        OFFLINE_COUNT=$((OFFLINE_COUNT+1))
        write_csv_row "$host" "$ip" "$SITE_ID" "$EXPECTED_TZ" "" "" "" "" \
            "UNKNOWN" "" "" "UNKNOWN" "UNKNOWN" "UNKNOWN" "UNKNOWN" "UNKNOWN" \
            "UNKNOWN" "UNKNOWN" "UNKNOWN" "UNKNOWN" "UNKNOWN" "UNKNOWN" "UNKNOWN" "UNKNOWN" "OFFLINE" "$RESULT"
        continue
    fi

    get_value() {
        printf '%s\n' "$RESULT" | awk -F= -v key="$1" '$1==key {sub(/^[^=]*=/,""); print; exit}'
    }

    DETECTED=$(get_value DETECTED_TZ)
    LINK=$(get_value LOCALTIME_LINK)
    TZDATA_VALUE=$(get_value TZDATA)
    TZJAVA_VALUE=$(get_value TZDATA_JAVA)
    PG_STATUS_VALUE=$(get_value PG_STATUS)
    PG_SERVICE_VALUE=$(get_value PG_SERVICE)
    PG_TZ=$(get_value PG_TIMEZONE)
    PG_LOG_TZ=$(get_value PG_LOG_TIMEZONE)
    SYS_MATCH=$(get_value SYSTEM_MATCH)
    TIMEZONE_RESULT_VALUE=$(get_value TIMEZONE_RESULT)
    PG_MATCH_VALUE=$(get_value PG_MATCH)
    DST_POLICY_VALUE=$(get_value DST_POLICY)
    ZDUMP_VALUE=$(get_value ZDUMP_STATUS)
    SPRING_2026_CLOCK_VALUE=$(get_value SPRING_2026_CLOCK_BEHAVIOR)
    FALL_2026_CLOCK_VALUE=$(get_value FALL_2026_CLOCK_BEHAVIOR)
    SPRING_2027_CLOCK_VALUE=$(get_value SPRING_2027_CLOCK_BEHAVIOR)
    FALL_2027_CLOCK_VALUE=$(get_value FALL_2027_CLOCK_BEHAVIOR)
    CLOCK_MATCH_VALUE=$(get_value CLOCK_MATCH)
    CLOCK_RESULT_VALUE=$(get_value CLOCK_RESULT)
    REVIEW_ONLY_VALUE=$(get_value REVIEW_ONLY)
    OVERALL_VALUE=$(get_value OVERALL)
    FAILURES_VALUE=$(get_value FAILURES)
    WARNINGS_VALUE=$(get_value WARNINGS)
    TARGET_VALUE=$(get_value LOCALTIME_TARGET)

    log "System timezone : ${DETECTED:-UNKNOWN}"
    log "localtime link   : ${LINK:-UNKNOWN} -> ${TARGET_VALUE:-UNKNOWN}"
    log "tzdata           : ${TZDATA_VALUE:-UNKNOWN}"
    log "tzdata-java      : ${TZJAVA_VALUE:-UNKNOWN}"
    log "PostgreSQL       : ${PG_STATUS_VALUE:-UNKNOWN} (${PG_SERVICE_VALUE:-unknown})"
    log "PG timezone      : ${PG_TZ:-UNKNOWN}"
    log "PG log_timezone  : ${PG_LOG_TZ:-UNKNOWN}"
    log "Timezone compare : ${TIMEZONE_RESULT_VALUE:-UNKNOWN} (system=${SYS_MATCH:-UNKNOWN}, PostgreSQL=${PG_MATCH_VALUE:-N/A})"
    log "DST policy       : ${DST_POLICY_VALUE:-UNKNOWN}"
    log "2026 spring clock: ${SPRING_2026_CLOCK_VALUE:-UNKNOWN}"
    log "2026 fall clock  : ${FALL_2026_CLOCK_VALUE:-UNKNOWN}"
    log "2027 spring clock: ${SPRING_2027_CLOCK_VALUE:-UNKNOWN}"
    log "2027 fall clock  : ${FALL_2027_CLOCK_VALUE:-UNKNOWN}"
    log "Clock compare    : ${CLOCK_RESULT_VALUE:-UNKNOWN}"
    log "zdump status     : ${ZDUMP_VALUE:-UNKNOWN}"
    log "Overall result   : ${OVERALL_VALUE:-UNKNOWN}"

    case "$TIMEZONE_RESULT_VALUE" in
        PASS) TIMEZONE_PASS_COUNT=$((TIMEZONE_PASS_COUNT+1)) ;;
        FAIL) TIMEZONE_FAIL_COUNT=$((TIMEZONE_FAIL_COUNT+1)) ;;
    esac
    case "$CLOCK_RESULT_VALUE" in
        PASS) CLOCK_PASS_COUNT=$((CLOCK_PASS_COUNT+1)) ;;
        FAIL) CLOCK_FAIL_COUNT=$((CLOCK_FAIL_COUNT+1)) ;;
    esac

    case "$OVERALL_VALUE" in
        PASS) PASS_COUNT=$((PASS_COUNT+1)) ;;
        FAIL) FAIL_COUNT=$((FAIL_COUNT+1)) ;;
        WARNING) WARN_COUNT=$((WARN_COUNT+1)) ;;
        *) WARN_COUNT=$((WARN_COUNT+1)) ;;
    esac

    DETAILS="failures=${FAILURES_VALUE:-?};warnings=${WARNINGS_VALUE:-?};target=${TARGET_VALUE:-unknown}"
    write_csv_row "$host" "$ip" "$SITE_ID" "$EXPECTED_TZ" "$DETECTED" "$LINK" \
        "$TZDATA_VALUE" "$TZJAVA_VALUE" "$PG_STATUS_VALUE" "$PG_SERVICE_VALUE" "$PG_TZ" "$PG_LOG_TZ" \
        "$SYS_MATCH" "$PG_MATCH_VALUE" "$TIMEZONE_RESULT_VALUE" "$DST_POLICY_VALUE" \
        "$SPRING_2026_CLOCK_VALUE" "$FALL_2026_CLOCK_VALUE" \
        "$SPRING_2027_CLOCK_VALUE" "$FALL_2027_CLOCK_VALUE" \
        "$CLOCK_MATCH_VALUE" "$CLOCK_RESULT_VALUE" \
        "$ZDUMP_VALUE" "$REVIEW_ONLY_VALUE" "$OVERALL_VALUE" "$DETAILS"
done

{
    echo "NETWORK TIMEZONE VALIDATION SUMMARY"
    echo "Date: $(date)"
    echo "Site: $SITE_ID"
    echo "Expected timezone: $EXPECTED_TZ"
    echo "Inventory database: $INVENTORY_DB_IP"
    echo "Scope: $SCOPE"
    if [ "$REVIEW_ONLY" -eq 1 ]; then
        echo "Mode: review-only (site timezone not enforced)"
    else
        echo "Mode: site timezone validation"
    fi
    echo
    echo "Total:      $TOTAL_COUNT"
    echo "PASS:       $PASS_COUNT"
    echo "FAIL:       $FAIL_COUNT"
    echo "WARNING:    $WARN_COUNT"
    echo "OFFLINE:    $OFFLINE_COUNT"
    echo
    echo "TIMEZONE COMPARISON"
    echo "PASS:       $TIMEZONE_PASS_COUNT"
    echo "FAIL:       $TIMEZONE_FAIL_COUNT"
    echo "CLOCK COMPARISON"
    echo "PASS:       $CLOCK_PASS_COUNT"
    echo "FAIL:       $CLOCK_FAIL_COUNT"
    echo
    echo "CSV: $CSV_FILE"
    echo "Log: $LOG_FILE"
} | tee "$SUMMARY_FILE"

log "Validation complete."

if [ "$FAIL_COUNT" -gt 0 ] || [ "$OFFLINE_COUNT" -gt 0 ]; then
    exit 1
elif [ "$WARN_COUNT" -gt 0 ]; then
    exit 2
else
    exit 0
fi
