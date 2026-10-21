# Deploying Time Zone Patch for Permanent Daylight Saving

**Author:** Sean Yu  
**Company:** Smiths Detection LLC  
**Email:** [sean.yu@smithsdetection.com](mailto:sean.yu@smithsdetection.com)

---

## 1. Overview

The `tz_update` utility updates the system timezone and, where applicable, PostgreSQL timezone settings. It can also install updated timezone database (`tzdata`) RPM packages.

---

## 2. Preparation

Before starting, confirm the following prerequisites:

- You have the approved `tz_update` binary obtained from Smiths technical support.
- You have access to a workstation connected to MUX or a standalone CTX system.
- The workstation can reach intended targets over SSH where remote updates are required.
- You have the required permissions to change settings and install RPM packages.
- If using **Option 2**, the required RPM files are available locally.

### RPM Package Requirements

Platform-specific RPMs must be placed in the same directory as the `tz_update` binary:
* **For Postman 1:** Use Fedora 20 (`fc20`) packages.
* **For Postman 4:** Use CentOS 7 (`el7`) packages.

---

## 3. Copy and Prepare the Utility

1. Copy `tz_update` (and optional platform-dependent RPM files) from the external USB flash drive to the workstation computer's `/home/geeds/Downloads/` folder.
2. Open a terminal and navigate to the directory:
   ```bash
   cd /home/geeds/Downloads
   ```
3. Check and update the file permissions:
   ```bash
   ls -l tz_update
   chmod 755 tz_update
   ls -l tz_update
   ```
4. Confirm execute permission is present (e.g., `-rwxr-xr-x`). Launch with the approved privilege level:
   ```bash
   sudo ./tz_update
   ```
5. Review the detected site, timezone, target list, package versions, and confirmation prompts before proceeding.

---

## 4. Run the Utility

Select one of the following menu options based on your deployment needs:

* **Option 1 — Automatic timezone detection and update**  
  *Default.* Use this option to apply automatically detected site timezone to all MUX network computers or standalone computers. No timezone database update will be carried out.  
  > ⚠️ **Note:** If the automatically detected timezone does not match the local site, **do not continue**—use Option 3 to manually select the correct timezone.

* **Option 2 — Install new tzdata package and update**  
  Use this option when the timezone database needs updating. Confirm RPMs match the target platform. Review package installation, timezone verification, and PostgreSQL results. If required RPMs cannot be found, check that RPM packages are in the same directory as the script and try again.

* **Option 3 — Manually select timezone and update**  
  Use this option when automatic detection is inaccurate or the site requires a specific timezone. Verify installed rules in [Section 7](#7-post-tzdata-update-validation).

* **Option 4 — Exit**  
  Select `4` to exit the utility.

---

## 5. Verify the System Timezone

Perform checks on each updated target either remotely from an SSH session or at the local console.

### 5.1 Confirm the Active Timezone

Run the following commands:
```bash
date
date '+%Y-%m-%d %H:%M:%S %Z %z'
date -u
ls -l /etc/localtime
readlink -f /etc/localtime
```
- Confirm local time and UTC offset match the intended site timezone.
- Confirm `/etc/localtime` is a symlink pointing to the correct timezone file.

### 5.2 Confirm Installed RPM Versions

Run the following commands to check package versions:
```bash
rpm -q tzdata
rpm -q tzdata-java
```

---

## 6. Verify PostgreSQL Settings

Perform these checks **only** on hosts where PostgreSQL databases are installed (such as the RAIN server and standalone CTX host computer).

1. Open a terminal and connect to the database instance:
   ```bash
   sudo -u postgres psql -d mux
   ```
2. At the `psql` prompt, run:
   ```sql
   SHOW TimeZone;
   SHOW log_timezone;
   ```
3. Confirm both output values match the intended timezone configuration.
4. Exit the PostgreSQL interface:
   ```sql
   \q
   ```

---

## 7. Post `tzdata` Update Validation

This section is required to validate the system post-new `tzdata` database update. The commands below use **Vancouver** as an example.

Confirm that installed rules do **not** move the local clock back one hour on **November 1, 2026**. The commands calculate future local times; they **do not** change the actual system clock.

### 7.1 Inspect Rules with `zdump`

```bash
zdump -v -c 2026,2027 America/Vancouver
```

Review output on November 1, 2026. The rules should not schedule a transition from UTC−7 to UTC−8. 

**Incorrect behavior example** (fallback is still present):
```text
America/Vancouver  Sun Nov  1 08:59:59 2026 UTC = Sun Nov  1 01:59:59 2026 PDT isdst=1 gmtoff=-25200
America/Vancouver  Sun Nov  1 09:00:00 2026 UTC = Sun Nov  1 01:00:00 2026 PST isdst=0 gmtoff=-28800
```

**Expected behavior** (no November fallback from UTC−7 to UTC−8; local time continues forward and offset remains `-0700`):
```text
America/Vancouver  Sun Nov  1 08:59:59 2026 UTC = Sun Nov  1 01:59:59 2026 PDT isdst=1 gmtoff=-25200
America/Vancouver  Sun Nov  1 09:00:00 2026 UTC = Sun Nov  1 02:00:00 2026 MST isdst=0 gmtoff=-25200
```
> ℹ️ *Note:* Abbreviations and formatting can vary; do not judge by PDT/PST labels alone. `zdump` checks the named zone, not whether `/etc/localtime` uses it; verify the configured zone separately.

### 7.2 Future-Time Test (Safe)

Run the following test commands:
```bash
TZ=America/Vancouver date -d '2026-11-01 08:30:00 UTC' '+%F %T %Z %z'
TZ=America/Vancouver date -d '2026-11-01 09:30:00 UTC' '+%F %T %Z %z'
```

**PASS (Permanent UTC−7)** — Expected output is similar to:
```text
2026-11-01 01:30:00 ... -0700
2026-11-01 02:30:00 ... -0700
```
Both offsets must be `-0700` and local time must advance from `01:30` to `02:30`. *(Note: The target timezone for your site may vary).*

---

## 8. Verify the Final Utility Summary

Review the final summary printed by `tz_update`:

- [ ] **Updated hosts:** Have successful update and verification results.
- [ ] **Failed hosts:** Have been investigated and documented.
- [ ] **Offline hosts:** Are recorded for retry when connectivity is restored.
- [ ] **Skipped hosts:** Are understood and were not expected to be processed.
- [ ] **Counts:** Are consistent with the inventory and host-level results.

> ⚠️ **Important:** An offline host must not be marked as successfully updated. For Vancouver sites, complete [Section 7](#7-post-tzdata-update-validation) before recording the host as compliant. Save console output with the maintenance record.
