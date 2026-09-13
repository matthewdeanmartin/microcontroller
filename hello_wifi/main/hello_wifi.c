/*
 * hello_wifi - minimal ESP32-S2 "hello world over WiFi".
 *
 * Joins your WiFi network as a client, then serves a web page on port 80.
 * Browse to the IP address printed on the serial monitor.
 */

#include <stdio.h>
#include <string.h>

#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/event_groups.h"

#include "esp_wifi.h"       // WiFi driver
#include "esp_event.h"      // event loop (WiFi/IP events arrive here)
#include "esp_log.h"        // ESP_LOGI etc -> serial monitor
#include "esp_system.h"     // esp_restart()
#include "esp_http_server.h"// tiny built-in HTTP server
#include "nvs_flash.h"      // non-volatile storage; WiFi driver needs it

/* Credentials come from menuconfig (idf.py menuconfig -> "Hello WiFi").
 * Keeping them out of source means they don't land in git. */
#define WIFI_SSID      CONFIG_HELLO_WIFI_SSID
#define WIFI_PASSWORD  CONFIG_HELLO_WIFI_PASSWORD
#define MAX_RETRIES    10

/* Tag prefixed to every log line, so you can filter the monitor output. */
static const char *TAG = "hello_wifi";

/* An "event group" is FreeRTOS's way to let one task block until another
 * signals it. We use it to wait for the connection to succeed or fail. */
static EventGroupHandle_t s_wifi_events;
#define WIFI_CONNECTED_BIT BIT0
#define WIFI_FAILED_BIT    BIT1

static int s_retry_count = 0;

/* Set once the first IP arrives, and never cleared. It switches the retry
 * policy from "fail fast" to "keep trying forever":
 *
 *   Before the first connection, giving up is useful - a typo'd SSID should
 *   report itself rather than retry silently forever.
 *
 *   After it, giving up is wrong. An AP rebooting, a brief outage, or routine
 *   housekeeping (reason 4, ASSOC_EXPIRE) are all recoverable, and a board
 *   left on a shelf should ride them out rather than needing a power cycle. */
static bool s_ever_connected = false;

/* Backoff between reconnect attempts, in milliseconds. Retrying flat-out is
 * pointless when an AP is rebooting, and it keeps the radio at full power. */
#define RECONNECT_DELAY_MIN_MS          1000
#define RECONNECT_DELAY_MAX_MS          30000   /* after a working connection */
#define RECONNECT_DELAY_STARTUP_MAX_MS  4000    /* before the first one */
static int s_reconnect_delay_ms = RECONNECT_DELAY_MIN_MS;

/* Reconnects after a delay. esp_wifi_connect() must not be called from a timer
 * callback context on some IDF versions, and we must never block inside the
 * event handler, so the sleep happens in its own short-lived task. */
static void reconnect_task(void *arg)
{
    vTaskDelay(pdMS_TO_TICKS(s_reconnect_delay_ms));

    /* Exponential backoff, capped, so a long outage settles into one attempt
     * every 30s instead of hammering the radio.
     *
     * The cap is lower before the first successful connection: startup retries
     * are bounded at MAX_RETRIES, so a long backoff would just delay reporting
     * a genuinely wrong password. Once connected, a patient 30s retry is the
     * right behaviour for riding out an AP reboot. */
    int cap = s_ever_connected ? RECONNECT_DELAY_MAX_MS
                               : RECONNECT_DELAY_STARTUP_MAX_MS;
    s_reconnect_delay_ms *= 2;
    if (s_reconnect_delay_ms > cap) {
        s_reconnect_delay_ms = cap;
    }

    esp_wifi_connect();
    vTaskDelete(NULL);   /* a task must delete itself; returning is a fault */
}

/* ------------------------------------------------------------------ */
/* WiFi event handling                                                 */
/* ------------------------------------------------------------------ */

/* Called by the event loop whenever WiFi or IP status changes.
 * This runs on a system task - keep it short, never block here. */
static void on_wifi_event(void *arg, esp_event_base_t base,
                          int32_t event_id, void *event_data)
{
    if (base == WIFI_EVENT && event_id == WIFI_EVENT_STA_START) {
        /* Driver is up and ready; now actually try to associate. */
        esp_wifi_connect();

    } else if (base == WIFI_EVENT && event_id == WIFI_EVENT_STA_DISCONNECTED) {
        /* Covers both "never connected" and "dropped later".
         * The reason code is what tells the three failure modes apart:
         *   201 NO_AP_FOUND   - SSID not seen (wrong name, or 5GHz-only, or range)
         *   202 AUTH_FAIL     - wrong password
         *   15  4WAY_TIMEOUT  - also usually wrong password
         *   205 CONNECTION_FAIL / 200 BEACON_TIMEOUT - weak or flaky signal */
        wifi_event_sta_disconnected_t *d =
            (wifi_event_sta_disconnected_t *) event_data;

        if (s_ever_connected) {
            /* Link lost after having worked. Retry indefinitely with backoff -
             * the network is presumably coming back. */
            ESP_LOGW(TAG, "link lost (reason %d); reconnecting in %d ms",
                     d->reason, s_reconnect_delay_ms);
            xTaskCreate(reconnect_task, "reconnect", 2048, NULL, 5, NULL);

        } else if (s_retry_count < MAX_RETRIES) {
            /* Never connected yet. Retry via the same backoff task rather than
             * calling esp_wifi_connect() straight away: reconnecting flat out
             * every couple of seconds can trip an access point's flood
             * protection, which then refuses the auth handshake and looks
             * exactly like a credentials problem. */
            s_retry_count++;
            ESP_LOGW(TAG, "disconnected (reason %d); retry %d/%d in %d ms",
                     d->reason, s_retry_count, MAX_RETRIES,
                     s_reconnect_delay_ms);
            xTaskCreate(reconnect_task, "reconnect", 2048, NULL, 5, NULL);

        } else {
            ESP_LOGE(TAG, "failed to connect after %d tries", MAX_RETRIES);
            xEventGroupSetBits(s_wifi_events, WIFI_FAILED_BIT);
        }

    } else if (base == IP_EVENT && event_id == IP_EVENT_STA_GOT_IP) {
        /* Associated AND got a DHCP lease - this is the real success signal. */
        ip_event_got_ip_t *event = (ip_event_got_ip_t *) event_data;
        ESP_LOGI(TAG, "got IP: " IPSTR, IP2STR(&event->ip_info.ip));

        s_retry_count = 0;
        s_reconnect_delay_ms = RECONNECT_DELAY_MIN_MS;  /* reset the backoff */
        s_ever_connected = true;

        xEventGroupSetBits(s_wifi_events, WIFI_CONNECTED_BIT);
    }
}

/* A startup network scan used to live here. It was removed: it was a
 * diagnostic for "is the AP in range?", it answered that question once, and it
 * caused more failures than it explained (a 14KB stack-allocated buffer that
 * overflowed the main task's 3.5KB stack, and scan/connect ordering issues).
 *
 * The connect path already reports what matters via the disconnect reason
 * code - 201 NO_AP_FOUND means "not seen", which is the same answer a scan
 * gives. Add a scan back only when actually debugging a range problem, and put
 * the wifi_ap_record_t array on the heap when doing so. */

/* Bring up WiFi in station mode and block until connected (or failed).
 * Returns true on success. */
static bool wifi_connect(void)
{
    s_wifi_events = xEventGroupCreate();

    /* Standard four-step bring-up of the networking stack: */
    ESP_ERROR_CHECK(esp_netif_init());                 // 1. TCP/IP stack
    ESP_ERROR_CHECK(esp_event_loop_create_default());  // 2. event loop
    esp_netif_create_default_wifi_sta();               // 3. station interface

    wifi_init_config_t init_cfg = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&init_cfg));         // 4. WiFi driver

    /* Subscribe before starting the driver, so the STA_START event that
     * esp_wifi_start() raises is what kicks off the connection. */
    ESP_ERROR_CHECK(esp_event_handler_instance_register(
        WIFI_EVENT, ESP_EVENT_ANY_ID, &on_wifi_event, NULL, NULL));
    ESP_ERROR_CHECK(esp_event_handler_instance_register(
        IP_EVENT, IP_EVENT_STA_GOT_IP, &on_wifi_event, NULL, NULL));

    wifi_config_t wifi_cfg = {
        .sta = {
            .ssid     = WIFI_SSID,
            .password = WIFI_PASSWORD,
        },
    };

    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_STA));
    ESP_ERROR_CHECK(esp_wifi_set_config(WIFI_IF_STA, &wifi_cfg));

    /* Power save is left at the ESP-IDF default (WIFI_PS_MIN_MODEM).
     *
     * Setting WIFI_PS_NONE looks appealing for a mains-powered HTTP server -
     * lower latency, faster to notice a dropped link - but it was the only
     * radio-level difference between a build that associated fine and one that
     * would not get past "state: init -> auth". Correlation, not proven cause,
     * but the default is known to work here and the latency gain is not worth
     * a connection that will not come up. */

    ESP_LOGI(TAG, "connecting to SSID \"%s\" ...", WIFI_SSID);
    ESP_ERROR_CHECK(esp_wifi_start());  /* raises STA_START -> esp_wifi_connect() */

    /* Sleep this task until the handler sets one of the two bits.
     * portMAX_DELAY = wait forever; pdFALSE = don't auto-clear bits. */
    EventBits_t bits = xEventGroupWaitBits(
        s_wifi_events,
        WIFI_CONNECTED_BIT | WIFI_FAILED_BIT,
        pdFALSE, pdFALSE, portMAX_DELAY);

    return (bits & WIFI_CONNECTED_BIT) != 0;
}

/* ------------------------------------------------------------------ */
/* HTTP server                                                         */
/* ------------------------------------------------------------------ */

/* Handler for GET / - returns a small HTML page.
 * Uptime comes from the FreeRTOS tick counter. */
static esp_err_t root_handler(httpd_req_t *req)
{
    char page[512];
    unsigned uptime_sec = (unsigned)(xTaskGetTickCount() / configTICK_RATE_HZ);

    snprintf(page, sizeof(page),
        "<!doctype html><html><head><meta charset=\"utf-8\">"
        "<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">"
        "<title>ESP32-S2</title></head>"
        "<body style=\"font-family:system-ui;max-width:32rem;margin:3rem auto;padding:0 1rem\">"
        "<h1>Hello from the ESP32-S2!</h1>"
        "<p>Served by your DiGiYes S2 Mini over WiFi.</p>"
        "<ul><li>Uptime: %u seconds</li>"
        "<li>Free heap: %lu bytes</li>"
        "<li>IDF version: %s</li></ul>"
        "</body></html>",
        uptime_sec,
        (unsigned long) esp_get_free_heap_size(),
        esp_get_idf_version());

    httpd_resp_set_type(req, "text/html");
    return httpd_resp_send(req, page, HTTPD_RESP_USE_STRLEN);
}

/* Plain-text endpoint - handy for curl and for scripting. */
static esp_err_t health_handler(httpd_req_t *req)
{
    httpd_resp_set_type(req, "text/plain");
    return httpd_resp_send(req, "ok\n", HTTPD_RESP_USE_STRLEN);
}

/* Browsers request /favicon.ico automatically. Without a handler the server
 * logs a 404 warning on every page load, which clutters the monitor.
 * 204 No Content is the polite "there isn't one, stop asking". */
static esp_err_t favicon_handler(httpd_req_t *req)
{
    httpd_resp_set_status(req, "204 No Content");
    return httpd_resp_send(req, NULL, 0);
}

static void start_webserver(void)
{
    httpd_config_t config = HTTPD_DEFAULT_CONFIG();
    httpd_handle_t server = NULL;

    if (httpd_start(&server, &config) != ESP_OK) {
        ESP_LOGE(TAG, "failed to start HTTP server");
        return;
    }

    /* Register the URI -> handler mappings. */
    httpd_uri_t root = {
        .uri = "/", .method = HTTP_GET, .handler = root_handler,
    };
    httpd_uri_t health = {
        .uri = "/health", .method = HTTP_GET, .handler = health_handler,
    };
    httpd_uri_t favicon = {
        .uri = "/favicon.ico", .method = HTTP_GET, .handler = favicon_handler,
    };
    httpd_register_uri_handler(server, &root);
    httpd_register_uri_handler(server, &health);
    httpd_register_uri_handler(server, &favicon);

    ESP_LOGI(TAG, "HTTP server listening on port %d", config.server_port);
}

/* ------------------------------------------------------------------ */
/* Entry point                                                         */
/* ------------------------------------------------------------------ */

/* app_main is ESP-IDF's equivalent of main(). It runs as a FreeRTOS task.
 * Returning from it is fine - the other tasks (WiFi, HTTP) keep running, which
 * is why the server survives "Returned from app_main()" in the log. */
void app_main(void)
{
    /* NVS holds WiFi calibration data; the driver requires it initialized.
     * If the partition is full or from an older version, wipe and retry. */
    esp_err_t err = nvs_flash_init();
    if (err == ESP_ERR_NVS_NO_FREE_PAGES ||
        err == ESP_ERR_NVS_NEW_VERSION_FOUND) {
        ESP_ERROR_CHECK(nvs_flash_erase());
        err = nvs_flash_init();
    }
    ESP_ERROR_CHECK(err);

    if (wifi_connect()) {
        start_webserver();
    } else {
        /* Startup failed outright. Reboot rather than sitting here inert: if
         * the cause was the AP being down at power-on, a later attempt will
         * succeed, and nobody has to walk over and pull the USB cable.
         *
         * Note this is only reachable before the first successful connection.
         * Once connected, the disconnect handler retries forever instead. */
        ESP_LOGE(TAG, "no WiFi - check SSID/password in 'idf.py menuconfig'");
        ESP_LOGE(TAG, "restarting in 30s ...");
        vTaskDelay(pdMS_TO_TICKS(30000));
        esp_restart();
    }
}
