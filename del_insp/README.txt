#
# Author: Sean Yu <sean.yu@smithsdetect.com>
# Maintainer: Sean Yu <sean.yu@smithsdetection.com>
#
# Internal Use Only – Authorized Smiths Detection employees and contractors only.
#
# This script is provided "as is" without warranty of any kind.
# Not an officially supported product unless explicitly stated.
#


## This script detects available inspections from MUXed and standalone CTXs and provide option to remove unused inspections.


## Usage

(command inside [] are to be entered as is)

1. Copy the binary executable file "del_insp" to USB flash drive.  
2. Plug USB flash drive into CTX, UI workstation or RAIN server.  
3. Login as lowercase geeds and open a terminal window.
4. At $ prompt, find out where USB drive is mounted: [ lsblk ]
5. At $ prompt, change working directory to USB drive:  [ cd /run/media/geeds/USBName ]
6. At $ prompt, run the binary file: [ ./del_insp ] and follow the onscreen prompt. 
   - Identify the number for CTX and associated HDE inspection to remove. 
   - Press the number and enter key to confirm deletion. The list will refresh.
   - When done, press 'q' to exit.
   - Note: Each CTX should have only one active inspection installed (HDE or HDEC). If the drop down list shows
         only one inspection available, do not remove that inspection otherwise machine will not operate properly! 
7. Umount USB flash drive [ $ umount /run/media/geeds/USBName ]
8. Remove USB flash drive.
9. Verify on CI that HDE option is now gone.
10. Document the work performed for future reference.


# For questions, issues, or improvement suggestions regarding this script,
please contact:

Sean Yu
Smiths Detection
sean.yu@smithsdetection.com
778-231-8868
