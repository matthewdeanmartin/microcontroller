idf_component_register(SRCS "main.c" "display.c" "status_led.c"
    INCLUDE_DIRS "." ".."
    REQUIRES esp_wifi esp_netif nvs_flash esp_http_server esp_timer esp_lcd
    esp_driver_spi esp_driver_gpio esp_driver_ledc esp_driver_rmt
    esp_driver_tsens esp_driver_sdspi fatfs sdmmc spiffs json esp_partition esp_app_format spi_flash)
spiffs_create_partition_image(storage ../storage FLASH_IN_PROJECT)
