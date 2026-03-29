## This script collects **CLEAR** bags from MUX and standalone CTXs and copies them to `bagdata_archive` folder in the USB flash drive where the script is invoked. See list below:

AIRPORT	LOCATION	PLATFORM	# of CLEAR images	Notes
YYZ 	DOM & INTL 	9800 		1000			DOM 600; INTL 400
YVR	TB O/S		5800		500			X133 400; X235 100
YUL	TB		9800		1000			K177/K178
YWG	DOM/INT		9800		500			K566/K567


## Installation

1. Copy the script file to external USB flash drive.  
2. Plug the drive into UI workstation.  
3. Find out where USB drive is mounted [ $ lsblk ]
3. Change to USB drive [ $ cd /run/media/geeds/USBName ]
4. Run the script directly on USB flash drive and follow the onscreen prompt:

    $./bag_collect	

   (if script couldnt' finish when using default 5-day range, try narrow down data range to one day. Most likely a single day will provide sufficient bags to meet the collection quota).
   (It's always a good idea to use slightly higher quota number as target. For example to collect 1000 bags, set quota to 1100. If script become stuck near the end say 1050 bags have been collected, simple press Ctrl-C to exit)
      
6. After bag collection is complete, review log file inside 'bagdata_archive' folder
7. Rename bagdata_archive to your airport code [ $ mv bagdata_archive YYZ ]
8. Umount USB flash drive [ $ umount /run/media/geeds/USBName ]
9. Contact Supervisor on how to send the USB drive to CATSA


# For questions, issues, or improvement suggestions regarding this script,
please contact:

Sean Yu
Smiths Detection
sean.yu@smithsdetection.com
778-231-8868
