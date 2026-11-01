# Deploying Time Zone Patch for Permanent Daylight Saving

**Author:** Sean Yu  
**Company:** Smiths Detection LLC  
**Email:** [sean.yu@smithsdetection.com](mailto:sean.yu@smithsdetection.com)

---

## 1. Overview

The `tz_update` utility is used to update Linux system timezone and, where applicable, PostgreSQL timezone settings. It can also install updated timezone database (`tzdata`) RPM packages. 

There are options to update:
- A single computer
- A standalone network
- MUX network components

---

## 2. Preparation

Before starting, confirm the following prerequisites:
- You have the approved `tz_update` binary obtained from Smiths technical support.
- You have the platform-specific RPMs (if updating the timezone database is required).
- You have access to a workstation connected to MUX or a standalone CTX system.
- The workstation can reach intended targets over SSH where remote updates are required.
- You have the required permissions to change settings and install RPM packages.
- If using **Option 2**, the required RPM files are available locally.

### File Requirements
* **Utility binary:** `tz_update`
* **Platform-specific RPMs:** Placed in the same directory as the `tz_update` binary (if updating timezone database).
  * **For Postman 1:**
    * `tzdata-2026e-1.fc20.noarch.rpm`
    * `tzdata-java-2026e-1.fc20.noarch.rpm`
  * **For Postman 4:**
    * `tzdata-2026e-1.el7.noarch.rpm`
    * *(Optional)* `tzdata-java-2026e-1.el7.noarch.rpm`

---

## 3. Copy and Execute the Utility

1. Copy `tz_update` (and optional platform-dependent RPM files) to a USB flash drive.
2. Log into one of the workstation computers, CTX host, or RAIN servers as lowercase `geeds`.
3. Plug in the USB flash drive to the computer.
4. Open a terminal and copy the files from the USB flash drive to the `/home/geeds/Downloads/` folder.  
   *(Example assumes USB flash drive name is `KINGSTON`)*:
   ```bash
   lsblk # Find where USB flash drive is mounted (e.g., /run/media/geeds/KINGSTON)
   cp /run/media/geeds/KINGSTON/tz_update /home/geeds/Downloads/
   cp /run/media/geeds/KINGSTON/tzdata-* /home/geeds/Downloads/
   ```

5. Verify files and modify permissions to ensure execute permissions are present (e.g., `-rwxr-xr-x`):
   ```bash
   cd /home/geeds/Downloads
   ls -l tz_update
   chmod 755 tz_update
   ls -l tz_update
   ```

6. Launch the utility with elevated privileges:
   ```bash
   sudo ./tz_update
   ```

---

## 4. Utility Main Menu

After launching the utility, the main menu options will be displayed:

* **1) Single computer:** Use this option to update the timezone on a single computer (local update only).
* **2) Standalone network:** Use this option to update a single CTX host and multiple UI workstations.
* **3) MUX network:** Use this option to update RAIN server, multiple CTX hosts, and UI workstations.

---

## 5. Utility Sub-Menu

After selecting an option from the main menu, the sub-menu options will appear:

* **1) Install new tzdata package, then select timezone:** Use when the timezone database needs updating. A new timezone database matching the target platform will be installed. Linux system timezone and PostgreSQL timezone settings (if applicable) will be updated.
* **2) Manually select timezone and update (no RPM update):** Use when automatic timezone detection is inaccurate or the site requires a specific timezone. Choose a relevant timezone from the list or enter a custom timezone from the IANA database. Linux system timezone and PostgreSQL timezone settings (if applicable) will be updated.
* **3) Return to main menu:** Returns to the previous main menu.
* **4) Exit:** Select `4` to exit the utility.

---

## 6. Verification Steps

### A. Verify Linux System Timezone
Confirm local time and UTC offset match the intended site timezone. Confirm `/etc/localtime` is a symlink pointing to the correct timezone file:

```bash
date
date '+%Y-%m-%d %H:%M:%S %Z %z'
date -u
ls -l /etc/localtime
readlink -f /etc/localtime
```

Confirm installed RPM versions:
```bash
rpm -q tzdata
rpm -q tzdata-java # If installed
```

### B. Verify PostgreSQL Timezone Settings
Perform these checks **only** on hosts where PostgreSQL database is installed (such as RAIN server and standalone CTX host computer). 

1. Connect to the database instance:
   ```bash
   sudo -u postgres PGPASSWORD=postgres psql -d mux
   ```
2. At the `psql` prompt, run the following commands (note the semicolons):
   ```sql
   show timezone; show log_timezone;
   ```
3. Confirm both output values match the intended timezone configuration.
4. Exit the PostgreSQL prompt:
   ```sql
   \q
   ```

---

## 7. Post Timezone Update Validation

Run the included validator script to confirm `tzdata` database updates and timezone rules:

```bash
chmod +x validator.sh
./validator.sh
```
Review the results to ensure timezone rules have updated correctly.

---

## 8. Final Summary Check

Review the final summary printed by `tz_update`:

- [ ] **Updated hosts:** Have successful update and verification results.
- [ ] **Failed hosts:** Have been investigated and documented.
- [ ] **Offline hosts:** Recorded for retry when connectivity is restored.
- [ ] **Skipped hosts:** Understood and were not expected to be processed.
- [ ] **Count consistency:** Counts match inventory and host-level results.

> **Important Notes:**
> * An offline host must **not** be marked as successfully updated. 
> * For Vancouver, complete **Section 7** before recording the host as compliant. 
> * Save console output along with the maintenance record.
