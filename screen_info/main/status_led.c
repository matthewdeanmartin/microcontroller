#include "status_led.h"
#include <stdatomic.h>
#include "driver/rmt_tx.h"
#include "driver/rmt_encoder.h"
#include "esp_timer.h"
#include "esp_system.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

static atomic_uint stage;
static atomic_bool ready, wifi, fatal;
static rmt_channel_handle_t channel;
static rmt_encoder_handle_t encoder;
void status_stage(unsigned n) { atomic_store(&stage,n); }
void status_ready(void) { atomic_store(&ready,true); }
void status_wifi(bool up) { atomic_store(&wifi,up); }
void status_fatal(void) { atomic_store(&fatal,true); }

static unsigned reset_flashes(void) {
    switch (esp_reset_reason()) {
    case ESP_RST_POWERON: return 1;
    case ESP_RST_SW: case ESP_RST_EXT: case ESP_RST_USB: return 2;
    case ESP_RST_PANIC: return 3;
    case ESP_RST_INT_WDT: case ESP_RST_TASK_WDT: case ESP_RST_WDT: return 4;
    case ESP_RST_BROWNOUT: return 5;
    default: return 6;
    }
}
static void run(void *arg) {
    unsigned flashes=reset_flashes();
    int64_t began=esp_timer_get_time()/1000;
    for (;;) {
        uint64_t t=esp_timer_get_time()/1000-began;
        unsigned r=0,g=0,b=0;
        if (atomic_load(&fatal)) {
            unsigned n=atomic_load(&stage);
            uint64_t p=t%(n*400+1600);
            if (p<n*400 && p%400<200) r=8;
        } else if (t<1500 || (t>=2000 && t<2000+flashes*300 && (t-2000)%300<100)) {
            r=g=b=5;
        } else if (t<2500+flashes*300) {
            // Gap between startup/reset marker and runtime health.
        } else if (!atomic_load(&ready)) {
            if(t%1000<500) {r=5;b=8;}
        } else if (!atomic_load(&wifi)) {
            if(t%1000<500) {r=8;g=3;}
        } else if(t%2000<100) g=8;
        uint8_t grb[3]={g,r,b};
        rmt_symbol_word_t symbols[25]={0};
        for(int i=0;i<24;i++) {
            bool bit=grb[i/8]&(0x80>>(i%8));
            symbols[i]=(rmt_symbol_word_t){.level0=1,.duration0=bit?8:3,.level1=0,.duration1=bit?4:9};
        }
        symbols[24]=(rmt_symbol_word_t){.duration0=1500,.duration1=1500};
        rmt_transmit_config_t tx={0};
        if(rmt_transmit(channel,encoder,symbols,sizeof(symbols),&tx)!=ESP_OK) vTaskDelete(NULL);
        // Keep the symbols alive until DMA/RMT is finished.
        if(rmt_tx_wait_all_done(channel,-1)!=ESP_OK) vTaskDelete(NULL);
        vTaskDelay(pdMS_TO_TICKS(50));
    }
}
esp_err_t status_led_init(void) {
    rmt_tx_channel_config_t cfg={.gpio_num=8,.clk_src=RMT_CLK_SRC_DEFAULT,
        .resolution_hz=10000000,.mem_block_symbols=48,.trans_queue_depth=1};
    esp_err_t e=rmt_new_tx_channel(&cfg,&channel); if(e) return e;
    rmt_copy_encoder_config_t enc={0};
    e=rmt_new_copy_encoder(&enc,&encoder); if(e) return e;
    e=rmt_enable(channel); if(e) return e;
    return xTaskCreate(run,"status-led",2048,NULL,2,NULL)==pdPASS?ESP_OK:ESP_ERR_NO_MEM;
}
