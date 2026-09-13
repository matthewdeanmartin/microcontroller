# Registers "main" as a component.
#   SRCS     - C files to compile
#   INCLUDE_DIRS - headers exposed to other components ("." = this dir)
#   REQUIRES - other components we call into
idf_component_register(
    SRCS "hello_wifi.c"
    INCLUDE_DIRS "."
    REQUIRES esp_wifi esp_netif nvs_flash esp_http_server
)
