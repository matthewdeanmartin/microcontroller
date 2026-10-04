#include <stdio.h>
#include <string.h>
#include <stdatomic.h>
#include "wifi_config.h"
#include "display.h"
#include "status_led.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/semphr.h"
#include "driver/gpio.h"
#include "driver/temperature_sensor.h"
#include "driver/sdspi_host.h"
#include "esp_wifi.h"
#include "esp_netif.h"
#include "esp_event.h"
#include "esp_http_server.h"
#include "esp_heap_caps.h"
#include "esp_timer.h"
#include "esp_system.h"
#include "esp_flash.h"
#include "esp_partition.h"
#include "esp_app_desc.h"
#include "esp_spiffs.h"
#include "esp_vfs_fat.h"
#include "esp_log.h"
#include "nvs_flash.h"
#include "sdmmc_cmd.h"
#include "cJSON.h"

static const char *TAG="screen_info";
static atomic_bool connected;
static atomic_bool joining;
static atomic_bool associated, memory_view;
static atomic_uint attempts, join_began, last_failed, join_timeouts;
static atomic_uint disconnects, disconnect_reason, post_stage;
static esp_netif_t *netif;
static temperature_sensor_handle_t sensor;
static bool storage_ok, sd_ok;
static uint32_t flash_size;
static SemaphoreHandle_t snapshot_lock;
static char snapshot[8192]="{}";
static const char *stage_names[]={"", "NVS", "LCD / SPI", "Temperature", "Flash storage", "Wi-Fi", "HTTP", "Monitor"};

static void check(esp_err_t err) {
    if(err==ESP_OK) return;
    ESP_LOGE(TAG,"POST stage %u (%s) failed: %s",atomic_load(&post_stage),stage_names[atomic_load(&post_stage)],esp_err_to_name(err));
    status_fatal();
    for(;;) vTaskDelay(pdMS_TO_TICKS(1000));
}
static void stage(unsigned n) {
    atomic_store(&post_stage,n); status_stage(n);
    ESP_LOGI(TAG,"POST %u: %s",n,stage_names[n]);
}
static uint32_t seconds_now(void) {return (uint32_t)(esp_timer_get_time()/1000000);}
static const char *wifi_reason(unsigned reason) {
    switch(reason) {
    case 0: return "None";
    case WIFI_REASON_NO_AP_FOUND: return "No AP found";
    case WIFI_REASON_AUTH_FAIL: return "Auth failed";
    case WIFI_REASON_ASSOC_FAIL: return "Assoc failed";
    case WIFI_REASON_HANDSHAKE_TIMEOUT: case WIFI_REASON_4WAY_HANDSHAKE_TIMEOUT: return "WPA timeout";
    case WIFI_REASON_BEACON_TIMEOUT: return "Beacon timeout";
    case WIFI_REASON_CONNECTION_FAIL: return "Connect failed";
    case WIFI_REASON_ASSOC_LEAVE: return "Local disconnect";
    default: return "See reason code";
    }
}
static void start_join(void) {
    atomic_store(&joining,true); atomic_store(&join_began,seconds_now());
    unsigned n=atomic_fetch_add(&attempts,1)+1;
    ESP_LOGI(TAG,"Wi-Fi attempt %u (60-second association/DHCP budget)",n);
    esp_err_t e=esp_wifi_connect();
    if(e!=ESP_OK) {
        atomic_store(&joining,false);atomic_store(&last_failed,seconds_now());
        ESP_LOGW(TAG,"Wi-Fi connect request failed: %s; retry in 5 seconds",esp_err_to_name(e));
    }
}
static void wifi_event(void *arg,esp_event_base_t base,int32_t id,void *data) {
    if(base==WIFI_EVENT && id==WIFI_EVENT_STA_START) {
        start_join();
    }
    if(base==WIFI_EVENT && id==WIFI_EVENT_STA_CONNECTED) atomic_store(&associated,true);
    if(base==WIFI_EVENT && id==WIFI_EVENT_STA_DISCONNECTED) {
        atomic_store(&connected,false); status_wifi(false);
        atomic_store(&joining,false);
        atomic_store(&associated,false);atomic_store(&last_failed,seconds_now());
        atomic_fetch_add(&disconnects,1);
        atomic_store(&disconnect_reason,((wifi_event_sta_disconnected_t *)data)->reason);
        ESP_LOGW(TAG,"Wi-Fi attempt %u disconnected: %s (%u); retry in 5 seconds",
            atomic_load(&attempts),wifi_reason(atomic_load(&disconnect_reason)),atomic_load(&disconnect_reason));
        // Monitor task retries at a bounded rate; event loop stays responsive.
    }
    if(base==IP_EVENT && id==IP_EVENT_STA_GOT_IP) {
        atomic_store(&connected,true); status_wifi(true);
        atomic_store(&joining,false);
        ESP_LOGI(TAG,"Wi-Fi joined after %u attempt(s), latest attempt took %u seconds",
            atomic_load(&attempts),(unsigned)(seconds_now()-atomic_load(&join_began)));
        ESP_LOGI(TAG,"Dashboard: http://" IPSTR "/",IP2STR(&((ip_event_got_ip_t *)data)->ip_info.ip));
    }
}
static void wifi_init(void) {
    check(esp_netif_init()); check(esp_event_loop_create_default());
    netif=esp_netif_create_default_wifi_sta(); if(!netif) check(ESP_ERR_NO_MEM);
    check(esp_netif_set_hostname(netif,"waveshare-c6"));
    wifi_init_config_t init=WIFI_INIT_CONFIG_DEFAULT(); check(esp_wifi_init(&init));
    check(esp_event_handler_register(WIFI_EVENT,ESP_EVENT_ANY_ID,wifi_event,NULL));
    check(esp_event_handler_register(IP_EVENT,IP_EVENT_STA_GOT_IP,wifi_event,NULL));
    wifi_config_t config={0};
    memcpy(config.sta.ssid,WIFI_SSID,strlen(WIFI_SSID));
    memcpy(config.sta.password,WIFI_PASSWORD,strlen(WIFI_PASSWORD));
    check(esp_wifi_set_storage(WIFI_STORAGE_RAM));
    check(esp_wifi_set_mode(WIFI_MODE_STA)); check(esp_wifi_set_config(WIFI_IF_STA,&config));
    check(esp_wifi_start());
}
static esp_err_t api(httpd_req_t *req) {
    char *copy=malloc(sizeof(snapshot)); if(!copy) return httpd_resp_send_err(req,HTTPD_500_INTERNAL_SERVER_ERROR,"No memory");
    xSemaphoreTake(snapshot_lock,portMAX_DELAY); memcpy(copy,snapshot,sizeof(snapshot)); xSemaphoreGive(snapshot_lock);
    httpd_resp_set_type(req,"application/json"); httpd_resp_set_hdr(req,"Cache-Control","no-store");
    esp_err_t e=httpd_resp_sendstr(req,copy); free(copy); return e;
}
static esp_err_t set_view(httpd_req_t *req) {
    bool memory=req->user_ctx!=NULL;
    atomic_store(&memory_view,memory);
    httpd_resp_set_type(req,"application/json");
    httpd_resp_set_hdr(req,"Cache-Control","no-store");
    return httpd_resp_sendstr(req,memory?"{\"view\":\"memory\"}":"{\"view\":\"live\"}");
}
static const char page[]=
"<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>"
"<title>Waveshare C6 · System info</title><style>body{font:16px system-ui;background:#101827;color:#e4eefa;margin:2rem auto;padding:0 1rem;max-width:850px}"
"h1{color:#55dbc3}button{padding:.7rem;background:#263b50;color:white;border:1px solid #55dbc3;border-radius:8px}"
"pre{white-space:pre-wrap;overflow-wrap:anywhere;background:#1a2638;padding:1rem;border-radius:12px}small{color:#abc}</style>"
"<h1>Waveshare C6</h1><p id=health>Connecting to dashboard…</p><button id=toggle>Memory &amp; storage</button>"
"<pre id=live></pre><pre id=storage hidden></pre><small>Readings update every second. Temperature is chip temperature, not room temperature."
" This LCD model uses the BOOT button to change screen views.</small><script>"
"const live=document.querySelector('#live'),storage=document.querySelector('#storage'),health=document.querySelector('#health');"
"document.querySelector('#toggle').onclick=async()=>{const memory=storage.hidden;try{const r=await fetch('/api/view/'+(memory?'memory':'live'),{method:'POST'});"
"if(!r.ok)throw Error(r.status);storage.hidden=!memory;document.querySelector('#toggle').textContent=memory?'Live screen':'Memory & storage';"
"}catch(e){health.textContent='Could not switch screen: '+e.message}};"
"async function refresh(){try{const r=await fetch('/api/status',{cache:'no-store'});if(!r.ok)throw Error(r.status);const d=await r.json();"
"health.textContent=(d.wifi.connected?'Wi-Fi connected':'Wi-Fi disconnected')+' · '+d.wifi.ip;"
"live.textContent=JSON.stringify({board:d.board,uptime_seconds:d.uptime_seconds,temperature_c:d.temperature_c,wifi:d.wifi,reset_reason:d.reset_reason,post:d.post},null,2);"
"storage.textContent=JSON.stringify({heap:d.heap,flash_bytes:d.flash_bytes,filesystem:d.filesystem,sd:d.sd,partitions:d.partitions},null,2);"
"}catch(e){health.textContent='Dashboard unavailable — retrying ('+e.message+')'}finally{setTimeout(refresh,1000)}}refresh();</script></html>";
static esp_err_t index_page(httpd_req_t *req) { httpd_resp_set_type(req,"text/html"); return httpd_resp_sendstr(req,page); }
static void http_init(void) {
    httpd_config_t cfg=HTTPD_DEFAULT_CONFIG(); cfg.stack_size=6144;
    httpd_handle_t server; check(httpd_start(&server,&cfg));
    httpd_uri_t root={.uri="/",.method=HTTP_GET,.handler=index_page};
    httpd_uri_t json={.uri="/api/status",.method=HTTP_GET,.handler=api};
    httpd_uri_t live={.uri="/api/view/live",.method=HTTP_POST,.handler=set_view};
    httpd_uri_t memory={.uri="/api/view/memory",.method=HTTP_POST,.handler=set_view,.user_ctx=(void *)1};
    check(httpd_register_uri_handler(server,&root)); check(httpd_register_uri_handler(server,&json));
    check(httpd_register_uri_handler(server,&live)); check(httpd_register_uri_handler(server,&memory));
}
static void line(int row,const char *text,uint16_t color) { check(display_line(row,text,color)); }
#define WHITE 0xffff
#define TEAL 0x4ef9
#define GOLD 0xfd60
static void monitor(void *arg) {
    bool last_button=false;
    int held_ticks=0;
    unsigned ticks=0;
    for(;;) {
        bool button=gpio_get_level(9)==0;
        if(button) held_ticks++; else held_ticks=0;
        if(held_ticks==3 && !last_button) {atomic_store(&memory_view,!atomic_load(&memory_view));last_button=true; ticks=0;}
        if(!button) last_button=false;
        if(ticks++%10!=0) {vTaskDelay(pdMS_TO_TICKS(100));continue;}
        bool memory=atomic_load(&memory_view);
        uint64_t uptime=esp_timer_get_time()/1000000;
        uint32_t now=seconds_now();
        if(!atomic_load(&connected) && atomic_load(&joining) && now-atomic_load(&join_began)>=60) {
            atomic_fetch_add(&join_timeouts,1);atomic_store(&joining,false);
            atomic_store(&last_failed,now);
            ESP_LOGW(TAG,"Wi-Fi attempt %u exceeded 60 seconds; disconnecting before retry",atomic_load(&attempts));
            esp_wifi_disconnect();
        }
        if(!atomic_load(&connected) && !atomic_load(&joining) && now-atomic_load(&last_failed)>=5) {
            start_join();
        }
        wifi_ap_record_t ap={0}; bool linked=atomic_load(&associated) && esp_wifi_sta_get_ap_info(&ap)==ESP_OK;
        esp_netif_ip_info_t ip={0}; esp_netif_get_ip_info(netif,&ip);
        char ipstr[16]; snprintf(ipstr,sizeof(ipstr),IPSTR,IP2STR(&ip.ip));
        if(!atomic_load(&connected)) strcpy(ipstr,"0.0.0.0");
        float temp=0; bool temp_ok=temperature_sensor_get_celsius(sensor,&temp)==ESP_OK;
        size_t total=heap_caps_get_total_size(MALLOC_CAP_8BIT),free_heap=heap_caps_get_free_size(MALLOC_CAP_8BIT);
        size_t minimum=heap_caps_get_minimum_free_size(MALLOC_CAP_8BIT),largest=heap_caps_get_largest_free_block(MALLOC_CAP_8BIT);
        size_t fs_total=0,fs_used=0;
        bool fs_info=storage_ok && esp_spiffs_info("storage",&fs_total,&fs_used)==ESP_OK;
        uint64_t sd_total=0,sd_free=0; bool sd_info=sd_ok && esp_vfs_fat_info("/sd",&sd_total,&sd_free)==ESP_OK;
        char buf[64];
        line(0,memory?"MEMORY & STORAGE":"WAVESHARE C6 / LIVE",TEAL);
        if(!memory) {
            if(atomic_load(&connected)) strcpy(buf,"Wi-Fi CONNECTED");
            else if(atomic_load(&associated)) strcpy(buf,"Wi-Fi WAIT DHCP");
            else snprintf(buf,sizeof(buf),"Wi-Fi try %u",atomic_load(&attempts));
            line(1,buf,atomic_load(&connected)?TEAL:GOLD);
            line(2,ipstr,WHITE);
            snprintf(buf,sizeof(buf),"RSSI %d dBm",ap.rssi); line(3,linked?buf:"RSSI unavailable",WHITE);
            snprintf(buf,sizeof(buf),"Channel %u",ap.primary); line(4,linked?buf:"Channel unavailable",WHITE);
            snprintf(buf,sizeof(buf),"Chip %.1f C",temp);line(5,temp_ok?buf:"Chip temp unavailable",WHITE);
            snprintf(buf,sizeof(buf),"Uptime %llu s",(unsigned long long)uptime);line(6,buf,WHITE);
            snprintf(buf,sizeof(buf),"Free RAM %u KiB",(unsigned)(free_heap/1024));line(7,buf,WHITE);
            snprintf(buf,sizeof(buf),"Min RAM %u KiB",(unsigned)(minimum/1024));line(8,buf,WHITE);
            snprintf(buf,sizeof(buf),"Join attempts %u",atomic_load(&attempts));line(9,buf,WHITE);
            snprintf(buf,sizeof(buf),"Last: %s",wifi_reason(atomic_load(&disconnect_reason)));line(10,buf,WHITE);
            if(atomic_load(&connected)) line(11,"POST complete / HTTP",TEAL);
            else if(atomic_load(&joining)) {
                snprintf(buf,sizeof(buf),"Joining %u / 60 s",(unsigned)(now-atomic_load(&join_began)));line(11,buf,GOLD);
            } else line(11,"Retry delay: 5 s",GOLD);
        } else {
            snprintf(buf,sizeof(buf),"RAM %u/%u KiB",(unsigned)(free_heap/1024),(unsigned)(total/1024));line(1,buf,WHITE);
            snprintf(buf,sizeof(buf),"Largest %u KiB",(unsigned)(largest/1024));line(2,buf,WHITE);
            snprintf(buf,sizeof(buf),"Flash %u KiB",(unsigned)(flash_size/1024));line(3,buf,WHITE);
            snprintf(buf,sizeof(buf),"FS free %u KiB",(unsigned)((fs_total-fs_used)/1024));line(4,fs_info?buf:"FS unavailable",WHITE);
            snprintf(buf,sizeof(buf),"FS total %u KiB",(unsigned)(fs_total/1024));line(5,fs_info?buf:"FS unavailable",WHITE);
            snprintf(buf,sizeof(buf),"SD free %llu MiB",(unsigned long long)(sd_free/(1024*1024)));line(6,sd_info?buf:"SD absent/unavailable",WHITE);
        }
        cJSON *d=cJSON_CreateObject();
        if(!d) {vTaskDelay(pdMS_TO_TICKS(100));continue;}
        cJSON_AddStringToObject(d,"board","Waveshare ESP32-C6-LCD-1.47");
        cJSON_AddStringToObject(d,"screen_view",memory?"memory":"live");
        cJSON_AddNumberToObject(d,"uptime_seconds",uptime);
        if(temp_ok)cJSON_AddNumberToObject(d,"temperature_c",temp);else cJSON_AddNullToObject(d,"temperature_c");
        cJSON_AddNumberToObject(d,"reset_reason",esp_reset_reason());cJSON_AddNumberToObject(d,"flash_bytes",flash_size);
        cJSON *post=cJSON_AddObjectToObject(d,"post");cJSON_AddNumberToObject(post,"stage",atomic_load(&post_stage));cJSON_AddBoolToObject(post,"complete",true);
        cJSON *w=cJSON_AddObjectToObject(d,"wifi");cJSON_AddBoolToObject(w,"connected",atomic_load(&connected));
        cJSON_AddStringToObject(w,"ip",ipstr);
        char ssid[33]={0}; memcpy(ssid,ap.ssid,32);cJSON_AddStringToObject(w,"ssid",linked?ssid:"");
        if(linked)cJSON_AddNumberToObject(w,"rssi_dbm",ap.rssi);else cJSON_AddNullToObject(w,"rssi_dbm");
        cJSON_AddNumberToObject(w,"channel",ap.primary);cJSON_AddNumberToObject(w,"disconnects",atomic_load(&disconnects));
        cJSON_AddNumberToObject(w,"last_disconnect_reason",atomic_load(&disconnect_reason));
        cJSON_AddStringToObject(w,"last_disconnect_description",wifi_reason(atomic_load(&disconnect_reason)));
        cJSON_AddNumberToObject(w,"join_attempts",atomic_load(&attempts));cJSON_AddNumberToObject(w,"join_timeouts",atomic_load(&join_timeouts));
        cJSON_AddNumberToObject(w,"attempt_elapsed_seconds",atomic_load(&joining)?now-atomic_load(&join_began):0);
        cJSON_AddNumberToObject(w,"attempt_timeout_seconds",60);
        cJSON_AddStringToObject(w,"state",atomic_load(&connected)?"connected":atomic_load(&associated)?"waiting_dhcp":atomic_load(&joining)?"joining":"retry_delay");
        cJSON *heap=cJSON_AddObjectToObject(d,"heap");cJSON_AddNumberToObject(heap,"total_bytes",total);cJSON_AddNumberToObject(heap,"free_bytes",free_heap);
        cJSON_AddNumberToObject(heap,"minimum_free_bytes",minimum);cJSON_AddNumberToObject(heap,"largest_block_bytes",largest);
        cJSON *fs=cJSON_AddObjectToObject(d,"filesystem");cJSON_AddBoolToObject(fs,"mounted",fs_info);
        cJSON_AddNumberToObject(fs,"total_bytes",fs_total);cJSON_AddNumberToObject(fs,"used_bytes",fs_used);cJSON_AddNumberToObject(fs,"free_bytes",fs_total-fs_used);
        cJSON *sd=cJSON_AddObjectToObject(d,"sd");cJSON_AddBoolToObject(sd,"mounted",sd_info);cJSON_AddNumberToObject(sd,"total_bytes",sd_total);cJSON_AddNumberToObject(sd,"free_bytes",sd_free);
        cJSON *parts=cJSON_AddArrayToObject(d,"partitions");
        esp_partition_iterator_t it=esp_partition_find(ESP_PARTITION_TYPE_ANY,ESP_PARTITION_SUBTYPE_ANY,NULL);
        int row=7;
        while(it) {
            const esp_partition_t *p=esp_partition_get(it);
            cJSON *entry=cJSON_CreateObject();cJSON_AddStringToObject(entry,"label",p->label);cJSON_AddNumberToObject(entry,"address",p->address);
            cJSON_AddNumberToObject(entry,"size_bytes",p->size);cJSON_AddNumberToObject(entry,"type",p->type);cJSON_AddNumberToObject(entry,"subtype",p->subtype);
            cJSON_AddItemToArray(parts,entry);
            if(memory && row<12) {snprintf(buf,sizeof(buf),"%s %u KiB",p->label,(unsigned)(p->size/1024));line(row++,buf,WHITE);}
            it=esp_partition_next(it);
        }
        esp_partition_iterator_release(it);
        if(memory) while(row<12) line(row++,"",WHITE);
        line(12,"BOOT: switch view",GOLD);
        char *json=cJSON_PrintUnformatted(d);cJSON_Delete(d);
        if(json) {xSemaphoreTake(snapshot_lock,portMAX_DELAY);strlcpy(snapshot,json,sizeof(snapshot));xSemaphoreGive(snapshot_lock);free(json);}
        vTaskDelay(pdMS_TO_TICKS(100));
    }
}
void app_main(void) {
    // LED initialized before POST so later failures can signal their stage.
    esp_err_t led=status_led_init();if(led) ESP_LOGW(TAG,"LED unavailable: %s",esp_err_to_name(led));
    stage(1); check(nvs_flash_init()); // Preserve existing NVS; never silently erase it.
    stage(2); check(display_init());
    stage(3);temperature_sensor_config_t tc=TEMPERATURE_SENSOR_CONFIG_DEFAULT(10,80);
    check(temperature_sensor_install(&tc,&sensor));check(temperature_sensor_enable(sensor));
    stage(4);check(esp_flash_get_size(NULL,&flash_size));
    esp_vfs_spiffs_conf_t fs={.base_path="/storage",.partition_label="storage",.max_files=2,.format_if_mount_failed=false};
    esp_err_t e=esp_vfs_spiffs_register(&fs); storage_ok=e==ESP_OK;
    if(!storage_ok) ESP_LOGW(TAG,"Filesystem unavailable: %s; flash initial storage image",esp_err_to_name(e));
    sdmmc_host_t host=SDSPI_HOST_DEFAULT();host.slot=SPI2_HOST;
    sdspi_device_config_t device=SDSPI_DEVICE_CONFIG_DEFAULT();device.host_id=SPI2_HOST;device.gpio_cs=4;
    esp_vfs_fat_sdmmc_mount_config_t mount={.format_if_mount_failed=false,.max_files=2,.allocation_unit_size=16*1024};
    sdmmc_card_t *card=NULL;e=esp_vfs_fat_sdspi_mount("/sd",&host,&device,&mount,&card);sd_ok=e==ESP_OK;
    if(!sd_ok) ESP_LOGW(TAG,"SD absent/unavailable: %s (optional)",esp_err_to_name(e));
    stage(5);wifi_init();
    stage(6);snapshot_lock=xSemaphoreCreateMutex();if(!snapshot_lock)check(ESP_ERR_NO_MEM);http_init();
    stage(7);gpio_config_t button={.pin_bit_mask=1ULL<<9,.mode=GPIO_MODE_INPUT,.pull_up_en=GPIO_PULLUP_ENABLE};check(gpio_config(&button));
    check(xTaskCreate(monitor,"monitor",8192,NULL,3,NULL)==pdPASS?ESP_OK:ESP_ERR_NO_MEM);
    status_ready();ESP_LOGI(TAG,"POST complete; Wi-Fi joining asynchronously");
}
