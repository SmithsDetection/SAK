# CLEAR Bag Collection Script

**Author:** Sean Yu\
**Company:** Smiths Detection LLC\
**Email:** sean.yu@smithsdetection.com  


**Confidentiality:** Internal Use Only — Authorized Smiths Detection employees and contractors only.

**Disclaimer:** This script is provided "as is" without warranty of any kind. It is not an officially supported product unless explicitly stated.

## 1. Overview

This script collects **CLEAR bags** from MUX and standalone CTXs and copies them to the `bagdata_archive` folder on the USB flash drive from which the script is invoked.

The collection targets are listed below.

| Airport | Location | Platform | Number of CLEAR Images | Notes |
|---|---|---|---:|---|
| YYZ | DOM & INTL | 9800 | 1,000 | DOM: 600; INTL: 400 |
| YVR | TB O/S | 5800 | 500 | X133: 400; X235: 100 |
| YUL | TB | 9800 | 1,000 | K177/K178 |
| YWG | DOM/INT | 9800 | 500 | K566/K567 |

## 2. Usage Instructions

Commands shown inside square brackets are to be entered as written, without the brackets.

1. **Copy the script to a USB flash drive.**

2. **Insert the USB flash drive** into a CTX, UI workstation, or RAIN server.

3. **Identify the USB mount location.**

   ```bash
   lsblk
   ```

4. **Change to the USB flash drive directory.**

   Replace `USBName` with the actual USB directory name.

   ```bash
   cd /run/media/geeds/USBName
   ```

5. **Run the script and follow the on-screen prompts.**

   ```bash
   ./bag_collect
   ```

   **Collection recommendations:**

   - If the script cannot finish using the default five-day date range, try narrowing the data range to one day. A single day will often provide sufficient bags to meet the collection quota.
   - Set the target quota slightly higher than the required number. For example, to collect 1,000 bags, set the quota to 1,100.
   - If the script becomes stuck near the end of collection—for example, after collecting approximately 1,050 bags—press `Ctrl+C` to exit.

6. **Review the collection log.**

   After collection is complete, review the log file inside the `bagdata_archive` folder.

7. **Rename the archive folder to your airport code.**

   For example, for Toronto Pearson (YYZ):

   ```bash
   mv bagdata_archive YYZ
   ```

8. **Unmount the USB flash drive.**

   Replace `USBName` with the actual USB mount directory.

   ```bash
   umount /run/media/geeds/USBName
   ```

9. **Arrange delivery of the USB flash drive.**

   Contact your supervisor for instructions on sending the USB drive to CATSA.

## 3. Support and Contact Information

For questions, issues, or improvement suggestions regarding this script, please contact author.


