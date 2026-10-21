use std::env;
use std::fs;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CONFIG_DB: &str = "/var/eds/edsconfigs.db";
const DB_USER: &str = "postgres";
const DB_NAME: &str = "mux";
const DB_PASSWORD: &str = "postgres";
const DB_PORT: &str = "5432";
const RAIN1: &str = "10.3.3.1";
const RAIN2: &str = "10.3.3.2";
const STANDALONE: &str = "10.3.40.1";
const SSH_TIMEOUT: u64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode { TimezoneOnly, Packages }

#[derive(Clone, Debug)]
struct Config {
    ssh_user: String,
    install_java: bool,
    mode: Mode,
    script_dir: PathBuf,
}

#[derive(Default, Debug)]
struct Summary {
    attempted: usize,
    succeeded: usize,
    updated: usize,
    failed: usize,
    offline: usize,
    skipped: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Host { name: String, ip: String }

fn run(program: &str, args: &[&str], input: Option<&[u8]>) -> io::Result<Output> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() { cmd.stdin(Stdio::piped()); }
    let mut child = cmd.spawn()?;
    if let Some(bytes) = input {
        if let Some(mut stdin) = child.stdin.take() { stdin.write_all(bytes)?; }
    }
    child.wait_with_output()
}

fn run_text(program: &str, args: &[&str], input: Option<&str>) -> Result<String, String> {
    let out = run(program, args, input.map(str::as_bytes)).map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{} {:?} failed ({}): {}", program, args,
            out.status, String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn log(message: &str) {
    let now = run_text("date", &["+%F %T"], None).unwrap_or_else(|_| "time-unknown".into());
    let host = run_text("hostname", &["-s"], None).unwrap_or_else(|_| "localhost".into());
    println!("[{now}] [{host}] {message}");
}

fn fail(message: &str) -> ! { eprintln!("ERROR: {message}"); std::process::exit(1) }

fn timestamp() -> String {
    run_text("date", &["+%Y%m%d%H%M%S"], None).unwrap_or_else(|_| {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs().to_string()
    })
}

fn validate_timezone(zone: &str) -> bool {
    !zone.is_empty() && !zone.starts_with('/') && !zone.contains("..") &&
        zone.chars().all(|c| c.is_ascii_alphanumeric() || "_+./-".contains(c))
}

fn timezone_for_site(site: &str) -> Option<&'static str> {
    match site.to_ascii_uppercase().as_str() {
        "YVR" | "YLW" | "YXS" | "YXX" | "YYJ" => Some("America/Vancouver"),
        "YEG" | "YMM" | "YYC" => Some("America/Edmonton"),
        "YQR" | "YXE" => Some("America/Regina"),
        "YWG" => Some("America/Winnipeg"),
        "YZF" => Some("America/Yellowknife"),
        "YXY" => Some("America/Whitehorse"),
        "YFB" => Some("America/Iqaluit"),
        "YHM" | "YHU" | "YKF" | "KMT" | "YOW" | "YQB" | "YQG" | "YSB" |
        "YTZ" | "YUL" | "YXU" | "YYZ" => Some("America/Toronto"),
        "YQT" => Some("America/Thunder_Bay"),
        "YFC" | "YQM" | "YSJ" | "YYG" => Some("America/Moncton"),
        "YHZ" => Some("America/Halifax"),
        "YQX" => Some("America/St_Johns"),
        _ => None,
    }
}

fn os_family(id: &str, version: &str) -> Option<&'static str> {
    if id == "fedora" && version.starts_with("20") { Some("fc20") }
    else if matches!(id, "centos" | "rhel" | "ol") && version.starts_with('7') { Some("el7") }
    else { None }
}

fn parse_inventory(text: &str) -> Vec<Host> {
    text.lines().filter_map(|line| {
        let (name, ip) = line.split_once('\t')?;
        let name = name.trim(); let ip = ip.trim();
        if name.is_empty() || ip.is_empty() { None }
        else { Some(Host { name: name.to_string(), ip: ip.to_string() }) }
    }).collect()
}

fn local_ips() -> Vec<String> {
    run_text("ip", &["-o", "-4", "addr", "show"], None).unwrap_or_default()
        .lines().filter_map(|line| {
            let address = line.split_whitespace().nth(3)?;
            Some(address.split('/').next()?.to_string())
        }).collect()
}

fn is_pg_server(ip: &str) -> bool { matches!(ip, RAIN1 | RAIN2 | STANDALONE) }

fn is_eds_machine(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let bytes = n.as_bytes();
    bytes.len() >= 7 && n.starts_with("host.") &&
        matches!(bytes[5], b'k' | b'x') && bytes[6..].iter().all(|c| c.is_ascii_digit())
}

fn needs_postgres_update(ip: &str, name: &str) -> bool {
    is_pg_server(ip) || is_eds_machine(name)
}

fn tcp_reachable(ip: &str) -> bool {
    let addrs = match (ip, 22).to_socket_addrs() { Ok(a) => a, Err(_) => return false };
    for addr in addrs {
        let addr: SocketAddr = addr;
        if TcpStream::connect_timeout(&addr, Duration::from_secs(SSH_TIMEOUT)).is_ok() { return true; }
    }
    false
}

fn ssh(
    config: &Config,
    host: &str,
    command: &str,
    stdin: Option<&str>,
) -> Result<String, String> {
    let target = format!("{}@{}", config.ssh_user, host);

    run_text(
        "ssh",
        &[
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=5",
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            &target,
            command,
        ],
        stdin,
    )
}

fn query_db(config: &Config, host: &str, sql: &str, local: &[String]) -> Result<String, String> {
    let psql = format!("PGPASSWORD='{}' psql -X -h 127.0.0.1 -p {} -U {} -d {} -At -w -v ON_ERROR_STOP=1",
        DB_PASSWORD, DB_PORT, DB_USER, DB_NAME);
    if local.iter().any(|ip| ip == host) {
        run_text("env", &["PGPASSWORD=postgres", "psql", "-X", "-h", "127.0.0.1", "-p", DB_PORT, "-U", DB_USER,
            "-d", DB_NAME, "-At", "-w", "-v", "ON_ERROR_STOP=1"], Some(sql))
            .map_err(|e| format!("local psql: {e}"))
    } else {
        if !tcp_reachable(host) {
            return Err(format!("SSH port {host}:22 is not reachable; skipping host-key discovery"));
        }
        ssh(config, host, &psql, Some(sql))
    }
}

fn connection_mode() -> Option<String> {
    if !Path::new(CONFIG_DB).is_file() { return None; }
    let tables = run_text("sqlite3", &[CONFIG_DB,
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%';"], None).ok()?;
    for table in tables.lines().filter(|s| !s.is_empty()) {
        let escaped = table.replace('"', "\"\"");
        let cols = run_text("sqlite3", &[CONFIG_DB, &format!("PRAGMA table_info(\"{escaped}\");")], None).ok()?;
        if cols.lines().any(|l| l.split('|').nth(1) == Some("connection_mode")) {
            let value = run_text("sqlite3", &[CONFIG_DB, &format!("SELECT connection_mode FROM \"{escaped}\" LIMIT 1;")], None).ok()?;
            if matches!(value.as_str(), "0" | "1" | "2") { return Some(value); }
        }
    }
    for table in tables.lines().filter(|s| !s.is_empty()) {
        let escaped = table.replace('"', "\"\"");
        let cols = run_text("sqlite3", &[CONFIG_DB, &format!("PRAGMA table_info(\"{escaped}\");")], None).ok()?;
        let mut key = None; let mut val = None;
        for row in cols.lines() {
            let p: Vec<&str> = row.split('|').collect(); if p.len() < 2 { continue; }
            if matches!(p[1], "key" | "name" | "config_name" | "parameter" | "setting") && key.is_none() { key = Some(p[1]); }
            if matches!(p[1], "value" | "config_value" | "parameter_value" | "setting_value") && val.is_none() { val = Some(p[1]); }
        }
        if let (Some(k), Some(v)) = (key, val) {
            let sql = format!("SELECT \"{v}\" FROM \"{escaped}\" WHERE lower(\"{k}\")='connection_mode' LIMIT 1;");
            if let Ok(value) = run_text("sqlite3", &[CONFIG_DB, &sql], None) {
                if matches!(value.as_str(), "0" | "1" | "2") { return Some(value); }
            }
        }
    }
    None
}

fn inventory(config: &Config, local: &[String]) -> Result<(String, String, Vec<Host>), String> {
    let local_server = local.iter().find(|ip| is_pg_server(ip)).cloned();
    let mut candidates: Vec<String> = Vec::new();
    if let Some(ip) = local_server { candidates.push(ip); }
    match connection_mode().as_deref() {
        Some("0") => candidates.extend([STANDALONE, RAIN1, RAIN2].map(str::to_string)),
        Some("1") | Some("2") | None => candidates.extend([RAIN1, RAIN2, STANDALONE].map(str::to_string)),
        _ => candidates.extend([RAIN1, RAIN2, STANDALONE].map(str::to_string)),
    }
    let mut unique = Vec::new();
    for ip in candidates { if !unique.contains(&ip) { unique.push(ip); } }
    let site_sql = "SELECT site_id FROM public.site_admin;";
    let inv_sql = "SELECT host_name || E'\\t' || host(host_ip) FROM public.host_admin WHERE host_name IS NOT NULL AND host_name <> '' AND host_ip IS NOT NULL AND host_ip <> '127.0.0.1'::inet ORDER BY host_ip;";
    for server in unique {
        log(&format!("Trying DB server {server} for site ID and inventory."));
        let site = match query_db(config, &server, site_sql, local) { Ok(v) => v, Err(e) => { log(&format!("Site ID query failed on {server}: {e}")); continue; } };
        let sites: Vec<&str> = site.lines().filter(|s| !s.trim().is_empty()).collect();
        if sites.len() != 1 { log(&format!("Expected one site_id row on {server}; received {}.", sites.len())); continue; }
        let site_id = sites[0].trim().to_ascii_uppercase();
        if site_id.is_empty() || !site_id.chars().all(|c| c.is_ascii_alphanumeric()) { log("Invalid site ID from DB."); continue; }
        let inv = match query_db(config, &server, inv_sql, local) { Ok(v) => v, Err(e) => { log(&format!("Inventory query failed on {server}: {e}")); continue; } };
        let hosts = parse_inventory(&inv);
        if hosts.is_empty() { log(&format!("Inventory from {server} is empty or invalid.")); continue; }
        log(&format!("Using site ID and inventory from {server}; site ID {site_id}."));
        return Ok((site_id, server, hosts));
    }
    Err("Could not retrieve a valid site ID and inventory from any DB server".into())
}

fn choose_timezone(site: &str) -> String {
    if let Some(zone) = timezone_for_site(site) {
        log(&format!("Site ID {site} maps to timezone {zone}."));
        zone.to_string()
    } else {
        log(&format!("No timezone mapping for site ID {site}; manual selection required."));
        select_timezone()
    }
}

fn choose_operation_mode(site: &str, suggested_zone: String) -> Option<(Mode, String)> {
    println!("\n============================================================\n EDS Network-wide Timezone Updater\n============================================================");
    println!("Site ID:  {site}\nTimezone: {suggested_zone}\n");
    println!("1) Automatic timezone detection and update (default)");
    println!("2) Install new tzdata package and update");
    println!("3) Manually select timezone and update");
    println!("4) Exit");
    print!("\nSelect an option [1]: "); let _ = io::stdout().flush();
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() { return None; }
    match answer.trim() {
        "" | "1" => Some((Mode::TimezoneOnly, suggested_zone)),
        "2" => Some((Mode::Packages, suggested_zone)),
        "3" => Some((Mode::TimezoneOnly, select_timezone())),
        "4" => None,
        other => {
            println!("Invalid selection: {other}");
            choose_operation_mode(site, suggested_zone)
        }
    }
}

fn select_timezone() -> String {
    let choices = ["America/Vancouver", "America/Edmonton", "America/Regina", "America/Winnipeg",
        "America/Yellowknife", "America/Whitehorse", "America/Iqaluit", "America/Toronto",
        "America/Thunder_Bay", "America/Moncton", "America/Halifax", "America/St_Johns"];
    println!("\nSelect timezone:");
    for (i, z) in choices.iter().enumerate() { println!("{:>2}) {z}", i + 1); }
    println!("13) Enter another IANA timezone");
    loop {
        print!("Choice [1-13]: "); let _ = io::stdout().flush();
        let mut line = String::new(); if io::stdin().read_line(&mut line).is_err() { continue; }
        let choice = line.trim();
        if let Ok(n) = choice.parse::<usize>() {
            if (1..=12).contains(&n) { return choices[n-1].to_string(); }
            if n == 13 {
                print!("IANA timezone: "); let _ = io::stdout().flush();
                let mut custom = String::new(); let _ = io::stdin().read_line(&mut custom);
                let custom = custom.trim().to_string(); if validate_timezone(&custom) { return custom; }
            }
        }
        println!("Invalid selection or timezone.");
    }
}

fn rpm_filename_matches(name: &str, family: &str, java: bool) -> bool {
    let prefix = if java { "tzdata-java-" } else { "tzdata-" };
    let suffix = format!(".{family}.noarch.rpm");
    name.starts_with(prefix) && name.ends_with(&suffix)
}

fn rpm_path(dir: &Path, family: &str, java: bool) -> Result<PathBuf, String> {
    let prefix = if java { "tzdata-java-" } else { "tzdata-" };
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("read RPM directory {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|x| x.to_str()) else { continue };
        if !rpm_filename_matches(name, family, java) || !path.is_file() { continue; }
        let query = run_text("rpm", &["-qp", "--qf", "%{NAME}|%{EPOCH}:%{VERSION}-%{RELEASE}|%{ARCH}", path.to_str().unwrap_or("")], None);
        let Ok(metadata) = query else { continue };
        let fields: Vec<&str> = metadata.split('|').collect();
        let expected_name = if java { "tzdata-java" } else { "tzdata" };
        let release_suffix = format!(".{family}");
        if fields.len() == 3 && fields[0] == expected_name && fields[2] == "noarch" &&
            !fields[1].is_empty() && fields[1].ends_with(release_suffix.as_str()) {
            candidates.push((fields[1].to_string(), path));
        }
    }
    if candidates.is_empty() {
        return Err(format!("no valid {prefix}*.{family}.noarch.rpm found in {}", dir.display()));
    }
    let mut best = candidates.remove(0);
    for candidate in candidates {
        if compare_rpm_versions(&candidate.0, &best.0)? == std::cmp::Ordering::Greater {
            best = candidate;
        }
    }
    Ok(best.1)
}

fn rpm_file_version(path: &Path) -> Result<String, String> {
    run_text("rpm", &["-qp", "--qf", "%{EPOCH}:%{VERSION}-%{RELEASE}", path.to_str().unwrap_or("")], None)
}

fn detect_local_os() -> Result<&'static str, String> {
    let s = fs::read_to_string("/etc/os-release").map_err(|e| format!("cannot read /etc/os-release: {e}"))?;
    let mut id = String::new(); let mut version = String::new();
    for line in s.lines() {
        if let Some(v) = line.strip_prefix("ID=") { id = v.trim_matches('"').to_string(); }
        if let Some(v) = line.strip_prefix("VERSION_ID=") { version = v.trim_matches('"').to_string(); }
    }
    os_family(&id, &version).ok_or_else(|| format!("unsupported local OS: {id} {version}"))
}

fn compare_rpm_versions(first: &str, second: &str) -> Result<std::cmp::Ordering, String> {
    let script = r#"import sys, rpm

def parse_evr(value):
    epoch, rest = value.split(':', 1) if ':' in value else ('0', value)
    if epoch in ('', '(none)', '(null)'):
        epoch = '0'
    version, release = rest.rsplit('-', 1)
    return epoch, version, release

try:
    c = rpm.labelCompare(parse_evr(sys.argv[1]), parse_evr(sys.argv[2]))
    print((c > 0) - (c < 0))
except Exception as e:
    sys.stderr.write(str(e) + '\n')
    sys.exit(2)
"#;
    let output = run_text("python", &["-c", script, first, second], None)
        .map_err(|e| format!("RPM EVR comparison failed (Python RPM bindings required): {e}"))?;
    match output.as_str() {
        "-1" => Ok(std::cmp::Ordering::Less),
        "0" => Ok(std::cmp::Ordering::Equal),
        "1" => Ok(std::cmp::Ordering::Greater),
        _ => Err(format!("unexpected RPM comparison result: {output}")),
    }
}

fn rpm_version(package: &str) -> Option<String> {
    run_text("rpm", &["-q", "--qf", "%{EPOCH}:%{VERSION}-%{RELEASE}", package], None).ok()
}

fn install_local_packages(config: &Config) -> Result<bool, String> {
    if config.mode == Mode::TimezoneOnly { log("Timezone-only mode: skipping RPM checks and installation."); return Ok(false); }
    let family = detect_local_os()?;
    let tzrpm = rpm_path(&config.script_dir, family, false)?;
    let javarpm = if config.install_java { Some(rpm_path(&config.script_dir, family, true)?) } else { None };
    let expected = rpm_file_version(&tzrpm)?;
    if let Some(java_path) = &javarpm {
        let java_version = rpm_file_version(java_path)?;
        if java_version != expected {
            return Err(format!("tzdata and tzdata-java RPM versions do not match: {expected} vs {java_version}"));
        }
    }
    let mut packages: Vec<PathBuf> = Vec::new();
    let installed = rpm_version("tzdata");
    let installed_java = if config.install_java { rpm_version("tzdata-java") } else { None };
    for (name, current) in [("tzdata", installed.as_deref()), ("tzdata-java", installed_java.as_deref())] {
        if name == "tzdata" || config.install_java {
            if let Some(current) = current {
                if compare_rpm_versions(current, &expected)? == std::cmp::Ordering::Greater {
                    log(&format!("WARNING: installed {name} ({current}) is newer than bundled version ({expected}); skipping RPM changes to prevent downgrade."));
                    return Ok(false);
                }
            }
        }
    }
    if installed.as_deref() == Some(expected.as_str()) {
        log(&format!("tzdata is already current ({expected})."));
    } else {
        log(&format!("tzdata needs update: installed={}, selected={expected}", installed.as_deref().unwrap_or("not installed")));
        packages.push(tzrpm.clone());
    }
    if let Some(java_path) = &javarpm {
        let installed = installed_java.clone();
        if installed.as_deref() == Some(expected.as_str()) {
            log(&format!("tzdata-java is already current ({expected})."));
        } else {
            log(&format!("tzdata-java needs update: installed={}, selected={expected}", installed.as_deref().unwrap_or("not installed")));
            packages.push(java_path.clone());
        }
    } else {
        log("Skipping tzdata-java installation (INSTALL_JAVA_TZDATA=0).");
    }
    if !packages.is_empty() {
        let mut args = vec!["-Uvh", "--replacepkgs"];
        let strings: Vec<String> = packages.iter().map(|p| p.to_string_lossy().to_string()).collect();
        args.extend(strings.iter().map(String::as_str));
        let out = run("rpm", &args, None).map_err(|e| e.to_string())?;
        if !out.status.success() { return Err(format!("RPM installation failed: {}", String::from_utf8_lossy(&out.stderr))); }
        log("Timezone RPM installation completed.");
    } else { log("All requested timezone RPM packages are already current."); }
    for (name, required) in [("tzdata", true), ("tzdata-java", config.install_java)] {
        if required && rpm_version(name).as_deref() != Some(expected.as_str()) { return Err(format!("{name} verification failed; expected {expected}")); }
    }
    Ok(!packages.is_empty())
}

fn set_local_timezone(zone: &str) -> Result<bool, String> {
    let target = PathBuf::from("/usr/share/zoneinfo").join(zone);
    if !target.is_file() { return Err(format!("timezone file not found: {}", target.display())); }
    let target_real = fs::canonicalize(&target).map_err(|e| e.to_string())?;
    let current_real = fs::canonicalize("/etc/localtime").ok();
    if current_real.as_ref() == Some(&target_real) { log(&format!("/etc/localtime already matches {zone}.")); return Ok(false); }
    let out = run("ln", &["-sfn", target.to_str().unwrap_or(""), "/etc/localtime"], None).map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(format!("cannot set /etc/localtime: {}", String::from_utf8_lossy(&out.stderr))); }
    if fs::canonicalize("/etc/localtime").ok().as_ref() != Some(&target_real) { return Err("/etc/localtime verification failed".into()); }
    log(&format!("Verified /etc/localtime -> {zone}.")); Ok(true)
}

fn rewrite_pg_conf(path: &Path, zone: &str) -> Result<bool, String> {
    let old = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut tz_done = false; let mut log_done = false; let mut changed = false;
    let mut output = String::new();
    for line in old.lines() {
        let trimmed = line.trim_start();
        let active = trimmed.strip_prefix('#').unwrap_or(trimmed).trim_start();
        if active.starts_with("log_timezone") && active["log_timezone".len()..].trim_start().starts_with('=') {
            if !log_done { output.push_str(&format!("log_timezone = '{zone}'\n")); log_done = true; changed = true; }
            else { changed = true; }
        } else if active.starts_with("timezone") && active["timezone".len()..].trim_start().starts_with('=') {
            if !tz_done { output.push_str(&format!("timezone = '{zone}'\n")); tz_done = true; changed = true; }
            else { changed = true; }
        } else { output.push_str(line); output.push('\n'); }
    }
    if !tz_done { output.push_str(&format!("timezone = '{zone}'\n")); changed = true; }
    if !log_done { output.push_str(&format!("log_timezone = '{zone}'\n")); changed = true; }
    if !changed { return Ok(false); }
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&temp, output).map_err(|e| e.to_string())?;
    fs::set_permissions(&temp, metadata.permissions()).map_err(|e| e.to_string())?;
    let temp_s = temp.to_string_lossy().to_string();
    let path_s = path.to_string_lossy().to_string();
    let chown = run("chown", &["--reference", &path_s, &temp_s], None).map_err(|e| e.to_string())?;
    if !chown.status.success() { let _ = fs::remove_file(&temp); return Err("could not preserve PostgreSQL config ownership".into()); }
    let chmod = run("chmod", &["--reference", &path_s, &temp_s], None).map_err(|e| e.to_string())?;
    if !chmod.status.success() { let _ = fs::remove_file(&temp); return Err("could not preserve PostgreSQL config permissions".into()); }
    fs::rename(&temp, path).map_err(|e| e.to_string())?;
    Ok(true)
}

fn update_local_postgres(zone: &str, ts: &str) -> Result<bool, String> {
    let (config, service) = if Path::new("/var/lib/pgsql/data/postgresql.conf").is_file() {
        (PathBuf::from("/var/lib/pgsql/data/postgresql.conf"), "postgresql.service")
    } else if Path::new("/var/lib/pgsql/10/data/postgresql.conf").is_file() {
        (PathBuf::from("/var/lib/pgsql/10/data/postgresql.conf"), "postgresql-10.service")
    } else { return Err("no recognized local PostgreSQL configuration found".into()); };
    let current = run_text("env", &["PGPASSWORD=postgres", "psql", "-X", "-h", "127.0.0.1", "-p", DB_PORT, "-U", DB_USER, "-d", DB_NAME, "-At", "-w", "-v", "ON_ERROR_STOP=1", "-c", "SHOW timezone;"], None)
        .map_err(|e| format!("could not read effective PostgreSQL timezone; no changes made: {e}"))?;
    let current_log = run_text("env", &["PGPASSWORD=postgres", "psql", "-X", "-h", "127.0.0.1", "-p", DB_PORT, "-U", DB_USER, "-d", DB_NAME, "-At", "-w", "-v", "ON_ERROR_STOP=1", "-c", "SHOW log_timezone;"], None)
        .map_err(|e| format!("could not read effective PostgreSQL log_timezone; no changes made: {e}"))?;
    if current == zone && current_log == zone { log("PostgreSQL timezone and log_timezone already correct."); return Ok(false); }
    let backup = config.with_file_name(format!("postgresql_{}.conf", ts));
    fs::copy(&config, &backup).map_err(|e| format!("backup PostgreSQL config: {e}"))?;
    log(&format!("PostgreSQL backup: {}", backup.display()));
    rewrite_pg_conf(&config, zone)?;
    if Path::new("/sbin/restorecon").exists() || Path::new("/usr/sbin/restorecon").exists() {
        let config_s = config.to_string_lossy().to_string();
        let out = run("restorecon", &[&config_s], None).map_err(|e| e.to_string())?;
        if !out.status.success() { return Err("could not restore PostgreSQL configuration SELinux label".into()); }
    }
    let active = run("systemctl", &["is-active", "--quiet", service], None).map(|o| o.status.success()).unwrap_or(false);
    if !active { return Err(format!("{service} is not active; configuration changed but not reloaded")); }
    let reload = run("systemctl", &["reload", service], None).map_err(|e| e.to_string())?;
    if !reload.status.success() { return Err("PostgreSQL reload failed".into()); }
    let effective = run_text("env", &["PGPASSWORD=postgres", "psql", "-X", "-h", "127.0.0.1", "-p", DB_PORT, "-U", DB_USER, "-d", DB_NAME, "-At", "-w", "-v", "ON_ERROR_STOP=1", "-c", "SHOW timezone;"], None).unwrap_or_default();
    let effective_log = run_text("env", &["PGPASSWORD=postgres", "psql", "-X", "-h", "127.0.0.1", "-p", DB_PORT, "-U", DB_USER, "-d", DB_NAME, "-At", "-w", "-v", "ON_ERROR_STOP=1", "-c", "SHOW log_timezone;"], None).unwrap_or_default();
    if effective != zone || effective_log != zone { return Err(format!("PostgreSQL verification failed: timezone={effective}, log_timezone={effective_log}")); }
    log(&format!("Verified PostgreSQL timezone and log_timezone: {zone}")); Ok(true)
}

const REMOTE_SCRIPT: &str = r#"
set -u
MODE="$1"; TZ="$2"; RPMDIR="$3"; IS_SERVER="$4"; INSTALL_JAVA="$5"; EXPECTED="$6"
DBUSER=postgres; DBNAME=mux; DBPASSWORD=postgres; DBPORT=5432
UPDATED=0
fail(){ echo "ERROR: $*" >&2; echo "EDS_TZ_UPDATED=$UPDATED"; exit 1; }
compare_versions(){
  python - "$1" "$2" <<'PYRPM'
import sys, rpm

def parse_evr(value):
    epoch, rest = value.split(':', 1) if ':' in value else ('0', value)
    if epoch in ('', '(none)', '(null)'):
        epoch = '0'
    version, release = rest.rsplit('-', 1)
    return epoch, version, release

try:
    c = rpm.labelCompare(parse_evr(sys.argv[1]), parse_evr(sys.argv[2]))
    print((c > 0) - (c < 0))
except Exception as exc:
    sys.stderr.write(str(exc) + '\n')
    sys.exit(2)
PYRPM
}
case "$MODE" in timezone|packages) ;; *) fail 'invalid mode';; esac
case "$TZ" in ''|/*|*..*|*[!A-Za-z0-9_+./-]*) fail 'invalid timezone';; esac
. /etc/os-release || fail 'cannot read /etc/os-release'
case "${ID:-}:${VERSION_ID:-}" in fedora:20*) FAMILY=fc20;; centos:7*|rhel:7*|ol:7*) FAMILY=el7;; *) fail "unsupported OS ${ID:-unknown} ${VERSION_ID:-unknown}";; esac
if [ "$MODE" = packages ]; then
  TZRPM="$RPMDIR/tzdata.rpm"; JAVARPM="$RPMDIR/tzdata-java.rpm"
  [ -n "$EXPECTED" ] || fail 'expected RPM version was not supplied'
  [ -f "$TZRPM" ] || fail "missing tzdata RPM $TZRPM"
  [ "$INSTALL_JAVA" != 1 ] || [ -f "$JAVARPM" ] || fail "missing tzdata-java RPM $JAVARPM"
  PACKAGES=(); INSTALLED_TZ=$(rpm -q --qf '%{EPOCH}:%{VERSION}-%{RELEASE}' tzdata 2>/dev/null || true)
  INSTALLED_JAVA=$(rpm -q --qf '%{EPOCH}:%{VERSION}-%{RELEASE}' tzdata-java 2>/dev/null || true)
  echo "Installed tzdata: ${INSTALLED_TZ:-not installed}"
  if [ "$INSTALL_JAVA" = 1 ]; then echo "Installed tzdata-java: ${INSTALLED_JAVA:-not installed}"; fi
  echo "Selected tzdata RPM version: $EXPECTED"
  if [ "$INSTALL_JAVA" = 1 ]; then echo "Selected tzdata-java RPM version: $EXPECTED"; fi
  SKIP_RPM=0
  for CURRENT in "$INSTALLED_TZ" "$([ "$INSTALL_JAVA" = 1 ] && printf '%s' "$INSTALLED_JAVA")"; do
    [ -n "$CURRENT" ] || continue
    CMP=$(compare_versions "$CURRENT" "$EXPECTED") || fail 'RPM EVR comparison failed; refusing package changes'
    if [ "$CMP" -gt 0 ]; then echo "WARNING: installed timezone data ($CURRENT) is newer than bundled version ($EXPECTED); skipping all RPM changes."; SKIP_RPM=1; fi
  done
  if [ "$SKIP_RPM" -eq 0 ]; then
    [ "$INSTALLED_TZ" = "$EXPECTED" ] || PACKAGES+=("$TZRPM")
    if [ "$INSTALL_JAVA" = 1 ]; then [ "$INSTALLED_JAVA" = "$EXPECTED" ] || PACKAGES+=("$JAVARPM"); fi
    if [ "${#PACKAGES[@]}" -gt 0 ]; then
      rpm -Uvh --replacepkgs "${PACKAGES[@]}" || fail 'RPM installation failed'
      UPDATED=1
    else
      echo "All requested timezone RPM packages are already current."
    fi
    [ "$(rpm -q --qf '%{EPOCH}:%{VERSION}-%{RELEASE}' tzdata 2>/dev/null || true)" = "$EXPECTED" ] || fail 'tzdata verification failed'
    if [ "$INSTALL_JAVA" = 1 ]; then [ "$(rpm -q --qf '%{EPOCH}:%{VERSION}-%{RELEASE}' tzdata-java 2>/dev/null || true)" = "$EXPECTED" ] || fail 'tzdata-java verification failed'; fi
  fi
fi
TARGET="/usr/share/zoneinfo/$TZ"; [ -f "$TARGET" ] || fail "timezone file missing: $TARGET"
RESOLVED=$(readlink -f "$TARGET") || fail 'cannot resolve timezone target'
CURRENT=$(readlink -f /etc/localtime 2>/dev/null || true)
if [ "$CURRENT" != "$RESOLVED" ]; then ln -sfn "$TARGET" /etc/localtime || fail 'cannot set /etc/localtime'; UPDATED=1; fi
[ "$(readlink -f /etc/localtime 2>/dev/null || true)" = "$RESOLVED" ] || fail 'timezone verification failed'
echo "Timezone verified: $TZ"
if [ "$IS_SERVER" = 1 ]; then
  if [ -f /var/lib/pgsql/data/postgresql.conf ]; then CONFIG=/var/lib/pgsql/data/postgresql.conf; SERVICE=postgresql.service
  elif [ -f /var/lib/pgsql/10/data/postgresql.conf ]; then CONFIG=/var/lib/pgsql/10/data/postgresql.conf; SERVICE=postgresql-10.service
  else fail 'designated DB server but PostgreSQL config not found'; fi
  EFFECTIVE=$(PGPASSWORD="$DBPASSWORD" psql -X -h 127.0.0.1 -p "$DBPORT" -U "$DBUSER" -d "$DBNAME" -At -w -v ON_ERROR_STOP=1 -c 'SHOW timezone;') || fail 'could not read effective PostgreSQL timezone; no changes made'
  LOG_EFFECTIVE=$(PGPASSWORD="$DBPASSWORD" psql -X -h 127.0.0.1 -p "$DBPORT" -U "$DBUSER" -d "$DBNAME" -At -w -v ON_ERROR_STOP=1 -c 'SHOW log_timezone;') || fail 'could not read effective PostgreSQL log_timezone; no changes made'
  if [ "$EFFECTIVE" = "$TZ" ] && [ "$LOG_EFFECTIVE" = "$TZ" ]; then echo "PostgreSQL timezone and log_timezone already correct"; else
  cp -p "$CONFIG" "${CONFIG%.conf}_$(date +%Y%m%d%H%M%S).conf" || fail 'PostgreSQL backup failed'
  TEMP="${CONFIG}.tmp.$$"
  awk -v tz="$TZ" 'BEGIN{t=0;l=0} /^[[:space:]]*#?[[:space:]]*log_timezone[[:space:]]*=/ {if(!l){printf "log_timezone = '\''%s'\''\n",tz;l=1};next} /^[[:space:]]*#?[[:space:]]*timezone[[:space:]]*=/ {if(!t){printf "timezone = '\''%s'\''\n",tz;t=1};next} {print} END{if(!t)printf "timezone = '\''%s'\''\n",tz;if(!l)printf "log_timezone = '\''%s'\''\n",tz}' "$CONFIG" > "$TEMP" || { rm -f "$TEMP"; fail 'cannot generate PostgreSQL config'; }
  chown --reference="$CONFIG" "$TEMP" 2>/dev/null || true; chmod --reference="$CONFIG" "$TEMP" 2>/dev/null || true
  mv -f "$TEMP" "$CONFIG" || fail 'cannot replace PostgreSQL config'
  UPDATED=1
  if command -v restorecon >/dev/null 2>&1; then restorecon "$CONFIG" || fail 'could not restore PostgreSQL configuration SELinux label'; fi
  systemctl is-active --quiet "$SERVICE" || fail "$SERVICE is not active; config changed but not reloaded"
  systemctl reload "$SERVICE" || fail 'PostgreSQL reload failed'
  EFFECTIVE=$(PGPASSWORD="$DBPASSWORD" psql -X -h 127.0.0.1 -p "$DBPORT" -U "$DBUSER" -d "$DBNAME" -At -w -v ON_ERROR_STOP=1 -c 'SHOW timezone;' 2>/dev/null || true)
  LOG_EFFECTIVE=$(PGPASSWORD="$DBPASSWORD" psql -X -h 127.0.0.1 -p "$DBPORT" -U "$DBUSER" -d "$DBNAME" -At -w -v ON_ERROR_STOP=1 -c 'SHOW log_timezone;' 2>/dev/null || true)
  [ "$EFFECTIVE" = "$TZ" ] && [ "$LOG_EFFECTIVE" = "$TZ" ] || fail "PostgreSQL verification failed: timezone=$EFFECTIVE log_timezone=$LOG_EFFECTIVE"
  echo "PostgreSQL timezone and log_timezone verified: $TZ"
  UPDATED=1
  fi
else
  echo "Not a designated PostgreSQL host; skipping PostgreSQL update."
fi
echo "EDS_TZ_UPDATED=$UPDATED"
"#;

fn update_remote(config: &Config, zone: &str, host: &Host, ts: &str) -> Result<bool, u8> {
    log(&format!(
        "[{}] Starting timezone update (mode={:?}).",
        host.name, config.mode
    ));

    if !tcp_reachable(&host.ip) {
        log(&format!(
            "[{}] OFFLINE/UNREACHABLE: SSH port {}:22 is not reachable.",
            host.name, host.ip
        ));
        return Err(3);
    }

    let os = match ssh(
        config,
        &host.ip,
        "if [ -r /etc/os-release ]; then . /etc/os-release; printf '%s:%s\\n' \"${ID:-}\" \"${VERSION_ID:-}\"; else exit 1; fi",
        None,
    ) {
        Ok(v) => v,
        Err(e) => {
            log(&format!("[{}] FAILED: OS detection: {e}", host.name));
            return Err(1);
        }
    };

    let (id, ver) = os.split_once(':').unwrap_or(("", ""));
    let family = match os_family(id, ver) {
        Some(f) => f,
        None => {
            log(&format!("[{}] FAILED: unsupported OS: {os}", host.name));
            return Err(1);
        }
    };

    log(&format!(
        "[{}] Detected OS: {os}; family: {family}",
        host.name
    ));

    let mut remote_dir = String::new();
    let mut expected = String::new();

    let target = format!("{}@{}", config.ssh_user, host.ip);

    let ssh_options = [
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=5",
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
    ];

    if config.mode == Mode::Packages {
        let base = match rpm_path(&config.script_dir, family, false) {
            Ok(p) => p,
            Err(e) => {
                log(&format!("[{}] FAILED: {e}", host.name));
                return Err(1);
            }
        };

        expected = match rpm_file_version(&base) {
            Ok(v) => v,
            Err(e) => {
                log(&format!(
                    "[{}] FAILED: cannot query RPM metadata: {e}",
                    host.name
                ));
                return Err(1);
            }
        };

        let java = if config.install_java {
            match rpm_path(&config.script_dir, family, true) {
                Ok(p) => match rpm_file_version(&p) {
                    Ok(v) if v == expected => Some(p),
                    Ok(v) => {
                        log(&format!(
                            "[{}] FAILED: tzdata ({expected}) and tzdata-java ({v}) RPM versions do not match",
                            host.name
                        ));
                        return Err(1);
                    }
                    Err(e) => {
                        log(&format!("[{}] FAILED: {e}", host.name));
                        return Err(1);
                    }
                },
                Err(e) => {
                    log(&format!("[{}] FAILED: {e}", host.name));
                    return Err(1);
                }
            }
        } else {
            None
        };

        remote_dir = format!(
            "/tmp/eds-tz-update-{ts}-{}",
            std::process::id()
        );

        let mkdir_cmd = format!("mkdir -m 700 '{remote_dir}'");
        let mut mk_args = ssh_options.to_vec();
        mk_args.push(&target);
        mk_args.push(&mkdir_cmd);

        let mk = run("ssh", &mk_args, None);

        if !mk.map(|o| o.status.success()).unwrap_or(false) {
            log(&format!(
                "[{}] FAILED: cannot create remote temp dir",
                host.name
            ));
            return Err(1);
        }

        let copy_one = |src: &Path, name: &str| -> bool {
            let dest = format!(
                "{}@{}:{}/{}",
                config.ssh_user, host.ip, remote_dir, name
            );

            let mut args = vec![
                "-q",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
            ];

            args.push(src.to_str().unwrap_or(""));
            args.push(&dest);

            run("scp", &args, None)
                .map(|o| o.status.success())
                .unwrap_or(false)
        };

        if !copy_one(&base, "tzdata.rpm")
            || (java
                .as_ref()
                .map_or(false, |p| !copy_one(p, "tzdata-java.rpm")))
        {
            let cleanup_command = format!("rm -rf '{remote_dir}'");
            let mut cleanup_args = ssh_options.to_vec();
            cleanup_args.push(&target);
            cleanup_args.push(&cleanup_command);

            let _ = run("ssh", &cleanup_args, None);

            log(&format!("[{}] FAILED: RPM transfer failed", host.name));
            return Err(1);
        }

        log(&format!(
            "[{}] RPM transfer completed.",
            host.name
        ));
    } else {
        log(&format!(
            "[{}] Timezone-only mode: skipping RPM checks and transfers.",
            host.name
        ));
    }

    let is_server = if needs_postgres_update(&host.ip, &host.name) {
        "1"
    } else {
        "0"
    };

    let mode = if config.mode == Mode::Packages {
        "packages"
    } else {
        "timezone"
    };

	let args = format!(
        "bash -s -- '{mode}' '{zone}' '{remote_dir}' '{is_server}' '{}' '{}'",
        if config.install_java { "1" } else { "0" },
        expected
    );

    let mut execute_args = ssh_options.to_vec();
    execute_args.push(&target);
    execute_args.push(&args);

    let output = run(
        "ssh",
        &execute_args,
        Some(REMOTE_SCRIPT.as_bytes()),
    );

    if !remote_dir.is_empty() {
        let cleanup_command = format!("rm -rf '{remote_dir}'");
        let mut cleanup_args = ssh_options.to_vec();
        cleanup_args.push(&target);
        cleanup_args.push(&cleanup_command);

        let _ = run("ssh", &cleanup_args, None);
    }

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let mut updated = false;

            for line in stdout.lines() {
                if line.trim() == "EDS_TZ_UPDATED=1" {
                    updated = true;
                } else if line.trim() != "EDS_TZ_UPDATED=0" {
                    log(&format!("[{}] {line}", host.name));
                }
            }

            Ok(updated)
        }

        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

            for line in stdout.lines().chain(stderr.lines()) {
                if !line.starts_with("EDS_TZ_UPDATED=") {
                    log(&format!("[{}] {line}", host.name));
                }
            }

            log(&format!(
                "[{}] FAILED: remote command exit {}",
                host.name, out.status
            ));

            Err(1)
        }

        Err(e) => {
            log(&format!(
                "[{}] FAILED: SSH execution: {e}",
                host.name
            ));
            Err(1)
        }
    }
}

fn self_test() -> bool {
    let cases = [
        ("YHU", Some("America/Toronto")), ("YYZ", Some("America/Toronto")),
        ("YVR", Some("America/Vancouver")), ("YWG", Some("America/Winnipeg")),
        ("YQX", Some("America/St_Johns")), ("UNKNOWN", None),
    ];
    for (site, expected) in cases { assert_eq!(timezone_for_site(site), expected, "site mapping {site}"); }
    assert!(validate_timezone("America/Toronto"));
    assert!(validate_timezone("Etc/UTC"));
    for bad in ["", "/etc/passwd", "America/../UTC", "America/Toronto;id"] { assert!(!validate_timezone(bad), "bad timezone {bad}"); }
    assert_eq!(os_family("fedora", "20"), Some("fc20"));
    assert_eq!(os_family("centos", "7.9"), Some("el7"));
    assert_eq!(os_family("rhel", "7.8"), Some("el7"));
    assert_eq!(os_family("centos", "8"), None);
    let inv = parse_inventory("TRI21\t10.3.4.21\n\nCI21\t10.3.4.22\nempty\t\n");
    assert_eq!(inv, vec![Host{name:"TRI21".into(),ip:"10.3.4.21".into()}, Host{name:"CI21".into(),ip:"10.3.4.22".into()}]);
    assert!(is_pg_server(RAIN1) && is_pg_server(RAIN2) && is_pg_server(STANDALONE));
    assert!(!is_pg_server("10.3.40.156"));
    assert!(is_eds_machine("host.K425"));
    assert!(is_eds_machine("HOST.x123"));
    assert!(!is_eds_machine("host.K"));
    assert!(!is_eds_machine("host.T425"));
    assert!(rpm_filename_matches("tzdata-2026e-1.el7.noarch.rpm", "el7", false));
    assert!(!rpm_filename_matches("tzdata-java-2026e-1.el7.noarch.rpm", "el7", false));
    assert!(rpm_filename_matches("tzdata-java-2026e-1.fc20.noarch.rpm", "fc20", true));
    assert!(!rpm_filename_matches("tzdata-2026e-1.el8.noarch.rpm", "el7", false));
    println!("PASS: site mapping, timezone validation, OS-family detection, inventory parsing, RPM filename matching, PostgreSQL host selection");
    true
}

fn parse_args() -> Result<(Config, bool), String> {
    let mut args = env::args().skip(1);
    let mut mode = Mode::TimezoneOnly;
    let mut user = env::var("SSH_USER").unwrap_or_else(|_| "root".into());
    let mut install_java = env::var("INSTALL_JAVA_TZDATA").unwrap_or_else(|_| "1".into()) == "1";
    let mut selftest = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--self-test" => selftest = true,
            "--mode" => mode = match args.next().as_deref() { Some("packages") => Mode::Packages, Some("timezone") => Mode::TimezoneOnly, _ => return Err("--mode requires packages or timezone".into()) },
            "--ssh-user" => user = args.next().ok_or("--ssh-user requires a value")?,
            "--no-java" => install_java = false,
            "--help" | "-h" => { println!("Usage: dst_tz_fix [--mode packages|timezone] [--ssh-user USER] [--no-java] [--self-test]\nThe interactive menu selects the operation; default is timezone-only. SSH_USER defaults to root; INSTALL_JAVA_TZDATA defaults to 1."); std::process::exit(0); },
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    let script_dir = env::current_exe().map_err(|e| e.to_string())?.parent().unwrap_or(Path::new(".")).to_path_buf();
    Ok((Config { ssh_user: user, install_java, mode, script_dir }, selftest))
}

fn main() {
    let (mut config, test) = parse_args().unwrap_or_else(|e| fail(&e));
    if test { self_test(); return; }
    if !Path::new("/etc/os-release").exists() { fail("cannot determine OS; /etc/os-release missing"); }
    let uid = run_text("id", &["-u"], None).unwrap_or_default();
    if uid != "0" { fail("run this program as root"); }
    if env::var("INSTALL_JAVA_TZDATA").map(|v| v != "0" && v != "1").unwrap_or(false) { fail("INSTALL_JAVA_TZDATA must be 0 or 1"); }

    let local = local_ips();
    let (site, source, hosts) = inventory(&config, &local).unwrap_or_else(|e| fail(&e));
    let suggested_zone = choose_timezone(&site);
    let Some((selected_mode, zone)) = choose_operation_mode(&site, suggested_zone) else {
        log("Exiting without making changes.");
        return;
    };
    config.mode = selected_mode;
    if !validate_timezone(&zone) { fail(&format!("invalid timezone syntax: {zone}")); }

    if config.mode == Mode::Packages {
        log(&format!("Discovering timezone RPMs in: {}", config.script_dir.display()));
        for family in ["fc20", "el7"] {
            let base_path = rpm_path(&config.script_dir, family, false)
                .unwrap_or_else(|e| fail(&e));
            let base_version = rpm_file_version(&base_path)
                .unwrap_or_else(|e| fail(&format!("cannot query {}: {e}", base_path.display())));
            log(&format!("{family} available tzdata: {base_version}"));

            if config.install_java {
                let java_path = rpm_path(&config.script_dir, family, true)
                    .unwrap_or_else(|e| fail(&e));
                let java_version = rpm_file_version(&java_path)
                    .unwrap_or_else(|e| fail(&format!("cannot query {}: {e}", java_path.display())));
                log(&format!("{family} available tzdata-java: {java_version}"));
                if base_version != java_version {
                    fail(&format!("tzdata and tzdata-java RPM versions differ for {family}: {base_version} vs {java_version}"));
                }
            }
        }
        log("RPM discovery completed.");
    }

    println!("\n============================================================\nSite ID:       {site}\nTimezone:      {zone}\nInventory DB:  {source}\nSSH user:      {}\nMode:          {}\nJava tzdata:   {}\n============================================================",
        config.ssh_user, if config.mode == Mode::Packages { "packages" } else { "timezone-only" }, if config.install_java { "1" } else { "0" });
    print!("Apply changes to this machine and inventory hosts? [Y/n]: "); let _ = io::stdout().flush();
    let mut answer = String::new(); let _ = io::stdin().read_line(&mut answer);
    if !answer.trim().is_empty() && !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") { log("Cancelled. No changes made."); return; }

    let ts = timestamp();
    let mut summary = Summary::default();
    summary.attempted = 1;
    log("Updating local machine...");
    let local_name = run_text("hostname", &["-s"], None).unwrap_or_else(|_| "localhost".into());
    let local_result = (|| -> Result<bool, String> {
        let mut changed = install_local_packages(&config)?;
        changed |= set_local_timezone(&zone)?;
        if needs_postgres_update(local.iter().find(|ip| is_pg_server(ip)).map(String::as_str).unwrap_or(""), &local_name) {
            changed |= update_local_postgres(&zone, &ts)?;
        } else { log("Local machine is not a designated PostgreSQL host; skipping PostgreSQL update."); }
        Ok(changed)
    })();
    match local_result {
        Ok(changed) => { summary.succeeded += 1; if changed { summary.updated += 1; } },
        Err(e) => { summary.failed += 1; log(&format!("Local machine update failed: {e}")); }
    }

    for host in hosts {
        if local.iter().any(|ip| ip == &host.ip) {
            log(&format!("[{}] Already processed locally; skipping duplicate remote pass.", host.name));
            continue;
        }
        summary.attempted += 1;
        match update_remote(&config, &zone, &host, &ts) {
            Ok(changed) => {
                summary.succeeded += 1;
                if changed { summary.updated += 1; }
                log(&format!("[{}] SUCCESS: Update completed ({}).", host.name, host.ip));
            },
            Err(3) => summary.offline += 1,
            Err(_) => { summary.failed += 1; log(&format!("[{}] Remote update failed; continuing to next host.", host.name)); }
        }
    }
    log(&format!("Finished processing inventory list: {} rows.", summary.attempted));
    println!("\n================ Timezone Update Summary ================\nSite ID:                  {site}\nSelected timezone:        {zone}\nDatabase source:           {source}\nHost total:                {}\nHost checked:              {}\nHost updated:              {}\nFailed hosts:              {}\nOffline hosts:             {}\nSkipped hosts:             {}\n==========================================================",
        summary.attempted, summary.succeeded, summary.updated, summary.failed, summary.offline, summary.skipped);
    if summary.failed > 0 { log(&format!("WARNING: {} machine(s) failed; review the output above.", summary.failed)); }
    if summary.offline > 0 { log(&format!("NOTICE: {} machine(s) were unreachable; they were not updated.", summary.offline)); }
    if summary.failed > 0 { std::process::exit(1); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn site_mapping() { assert_eq!(timezone_for_site("YHU"), Some("America/Toronto")); }
    #[test] fn timezone_input_validation() { assert!(validate_timezone("America/Toronto")); assert!(!validate_timezone("/etc/passwd")); assert!(!validate_timezone("../UTC")); }
    #[test] fn family_detection() { assert_eq!(os_family("fedora", "20"), Some("fc20")); assert_eq!(os_family("centos", "7.8.2003"), Some("el7")); assert_eq!(os_family("centos", "8"), None); }
    #[test] fn inventory_parse() { assert_eq!(parse_inventory("TRI21\t10.3.4.21\nCI21\t10.3.4.22\n" ).len(), 2); }
    #[test] fn eds_host_detection() { assert!(is_eds_machine("host.K425")); assert!(is_eds_machine("host.X1")); assert!(!is_eds_machine("TRI21")); }
}
