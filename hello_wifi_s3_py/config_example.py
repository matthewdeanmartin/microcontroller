"""Copy this file to config.py and fill in your details.

config.py is gitignored so credentials stay out of version control.
"""

# Hostname the board advertises over mDNS -> http://<HOSTNAME>.local
HOSTNAME = "esp32s3"

# The on-board addressable RGB LED (WS2812). GPIO48 on this board, confirmed
# from the factory demo preserved in reference/factory_neopixel_demo.py.
# Other S3 devkits use 38. Set to None to disable the LED entirely.
NEOPIXEL_PIN = 48

# Hours from UTC, for displaying wall-clock time. MicroPython has no tzdata,
# so this is a fixed offset and will be an hour out across a DST boundary.
# US Eastern: -5 winter, -4 summer.
UTC_OFFSET_HOURS = -4

WIFI_SSID = "your-network-name"
WIFI_PASSWORD = "your-password"
