# Deploying Time Zone Patch for Permanent Daylight Saving

**Author:** Sean Yu\
**Company:** Smiths Detection LLC\
**Email:** sean.yu@smithsdetection.com

## 1. Overview

The `tz_update` utility updates the system timezone and, where
applicable, PostgreSQL timezone settings. It can also install updated
timezone database (`tzdata`) RPM packages.

## 2. Preparation

Before starting, confirm:

-   You have the approved `tz_update` binary obtained from Smiths
    technical support.
-   You have the platform-specific RPMs (if updating the timezone
    database is required).
-   You have access to a workstation connected to MUX or a standalone
    CTX system.
-   The workstation can reach intended targets over SSH where remote
    updates are required.
-   You have the required permissions to change settings and install RPM
    packages.
-   If using Option 2, the required RPM files are available locally.

### Files requirements

-   Utility binary: **`tz_update`**

-   Platform-specific RPMs in the same directory as the `tz_update`
    binary (if updating the timezone database):

    -   For Postman 1: `tzdata-2026e-1.fc20.noarch.rpm`\
        Optional: `tzdata-java-2026e-1.fc20.noarch.rpm`
    -   For Postman 4: `tzdata-2026e-1.el7.noarch.rpm`\
        Optional: `tzdata-java-2026e-1.el7.noarch.rpm`

## 3. Copy and execute the utility

1.  Copy `tz_update` (and optional platform-dependent RPM files) to a
    USB flash drive.

2.  Log in to a workstation computer, CTX host, or RAIN server as the
    lowercase user `geeds`.

3.  Plug the USB flash drive into that computer.

4.  Open a terminal and enter the following commands to copy files from
    the USB flash drive to `/home/geeds/Downloads/`. This example
    assumes the USB flash drive is named `KINGSTON`.

    Find where the USB flash drive is mounted (for example,
    `/run/media/geeds/KINGSTON`):

    ``` bash
    lsblk
    ```

    Copy the utility and any bundled timezone RPMs:

    ``` bash
    cp /run/media/geeds/KINGSTON/tz_update /home/geeds/Downloads/
    cp /run/media/geeds/KINGSTON/tzdata-* /home/geeds/Downloads/
    ```

5.  Verify the files and set permissions. Confirm that execute
    permission is present (for example, `-rwxr-xr-x`):

    ``` bash
    cd /home/geeds/Downloads
    ls -l tz_update
    chmod 755 tz_update
    ls -l tz_update
    ```

6.  Run the utility with the approved privilege level:

    ``` bash
    sudo ./tz_update
    ```

## 4. Utility options

### Option 1 --- Automatic timezone detection and update

**Menu entry:** `1) Automatic timezone detection and update (default)`

Use this option to automatically detect the site timezone and apply it
to all MUX network computers or standalone computers. The Linux system
timezone and PostgreSQL timezone settings (if applicable) will be
updated. No timezone database update will be carried out.

If the automatically detected timezone does not match the local site, do
not continue. Use Option 3 to manually select the correct timezone.

### Option 2 --- Install new tzdata package and update

**Menu entry:** `2) Install new tzdata package and update`

Use this option when the timezone database needs updating. The new
timezone database package matching the target platform will be
installed. The Linux system timezone and PostgreSQL timezone settings
(if applicable) will be updated.

### Option 3 --- Manually select timezone and update

**Menu entry:** `3) Manually select timezone and update`

Use this option when automatic timezone detection is inaccurate or the
site requires a specific timezone. Choose a relevant timezone from the
list or enter a custom timezone found in the IANA database. The Linux
system timezone and PostgreSQL timezone settings (if applicable) will be
updated.

### Option 4 --- Exit

**Menu entry:** `4) Exit`

Select Option 4 to exit the utility.

## 5. Verify Linux system timezone

Confirm that the local time and UTC offset match the intended site
timezone. Confirm that `/etc/localtime` is a symlink pointing to the
correct timezone file.

``` bash
date
date '+%Y-%m-%d %H:%M:%S %Z %z'
date -u
ls -l /etc/localtime
readlink -f /etc/localtime
```

Confirm the installed RPM versions:

``` bash
rpm -q tzdata
rpm -q tzdata-java
```

Run `rpm -q tzdata-java` only if the Java timezone package is installed.

## 6. Verify PostgreSQL settings

Perform these checks only on hosts where PostgreSQL databases are
installed, such as a RAIN server or standalone CTX host. Open a terminal
and connect to the database instance:

``` bash
sudo -iu postgres psql -d mux
```

At the `psql` prompt, enter:

``` sql
SHOW timezone;
SHOW log_timezone;
```

Confirm that both output values match the intended timezone
configuration, then exit `psql`:

``` text
\q
```

## 7. Post-timezone-database-update validation

This section is required to validate the timezone database after an
update. The commands below use Vancouver as an example. Confirm that the
installed rules do not move the local clock back one hour on November 1,
2026. These commands calculate future local times; they do not change
the actual system clock.

### 7.1 Inspect rules with `zdump`

``` bash
zdump -v -c 2026,2027 America/Vancouver
```

Review the output for November 1, 2026. The rules should not schedule a
transition from UTC−7 to UTC−8.

The following output illustrates **incorrect rules**, where the fallback
is still present:

``` text
America/Vancouver  Sun Nov  1 08:59:59 2026 UTC = Sun Nov  1 01:59:59 2026 PDT isdst=1 gmtoff=-25200
America/Vancouver  Sun Nov  1 09:00:00 2026 UTC = Sun Nov  1 01:00:00 2026 PST isdst=0 gmtoff=-28800
```

**Expected behavior:** no November fallback from UTC−7 to UTC−8. Local
time continues forward and the offset remains `-0700`. Abbreviations and
formatting can vary; do not judge by PDT/PST labels alone. `zdump`
checks the named timezone, not whether `/etc/localtime` uses it. Verify
the configured zone separately.

Expected output should show no clock rollback and an unchanged UTC
offset, for example:

``` text
America/Vancouver  Sun Nov  1 08:59:59 2026 UTC = Sun Nov  1 01:59:59 2026 PDT isdst=1 gmtoff=-25200
America/Vancouver  Sun Nov  1 09:00:00 2026 UTC = Sun Nov  1 02:00:00 2026 MST isdst=0 gmtoff=-25200
```

### 7.2 Future-time test (does not change the system clock)

Run:

``` bash
TZ=America/Vancouver date -d '2026-11-01 08:30:00 UTC' '+%F %T %Z %z'
TZ=America/Vancouver date -d '2026-11-01 09:30:00 UTC' '+%F %T %Z %z'
```

**PASS --- permanent UTC−7.** Expected output is similar to:

``` text
2026-11-01 01:30:00 ... -0700
2026-11-01 02:30:00 ... -0700
```

Both offsets must be `-0700`, and local time must advance from 01:30 to
02:30. The timezone for your site may vary.

## 8. Verify the utility final summary

Review the final summary printed by `tz_update`.

-   Updated hosts have successful update and verification results.
-   Failed hosts have been investigated and documented.
-   Offline hosts are recorded for retry when connectivity is restored.
-   Skipped hosts are understood and were not expected to be processed.
-   Counts are consistent with the inventory and host-level results.

An offline host must not be marked successfully updated. For Vancouver,
complete Section 7 before recording the host as compliant. Save the
console output with the maintenance record.
