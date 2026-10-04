#include "display.h"
#include <string.h>
#include "font.h"
#include "driver/spi_master.h"
#include "driver/gpio.h"
#include "driver/ledc.h"
#include "esp_lcd_panel_io.h"
#include "esp_lcd_panel_ops.h"
#include "esp_lcd_panel_vendor.h"
#include "esp_heap_caps.h"
#include "freertos/FreeRTOS.h"
#include "freertos/semphr.h"

static esp_lcd_panel_handle_t panel;
static uint16_t *pixels;
static SemaphoreHandle_t done;
static bool flushed(esp_lcd_panel_io_handle_t io, esp_lcd_panel_io_event_data_t *event,void *ctx) {
    BaseType_t wake=pdFALSE;
    xSemaphoreGiveFromISR(done,&wake);
    return wake==pdTRUE;
}
#define TRY(x) do {esp_err_t e=(x); if(e) return e;} while(0)
esp_err_t display_init(void) {
    // SD card shares this SPI bus. Deselect it before sending LCD traffic.
    TRY(gpio_set_direction(4,GPIO_MODE_OUTPUT)); TRY(gpio_set_level(4,1));
    spi_bus_config_t bus={.mosi_io_num=6,.miso_io_num=5,.sclk_io_num=7,
        .quadwp_io_num=-1,.quadhd_io_num=-1,.max_transfer_sz=172*24*2};
    TRY(spi_bus_initialize(SPI2_HOST,&bus,SPI_DMA_CH_AUTO));
    done=xSemaphoreCreateBinary();
    pixels=heap_caps_malloc(172*24*2,MALLOC_CAP_DMA);
    if(!done || !pixels) return ESP_ERR_NO_MEM;
    esp_lcd_panel_io_handle_t io;
    esp_lcd_panel_io_spi_config_t iocfg={.cs_gpio_num=14,.dc_gpio_num=15,
        .spi_mode=0,.pclk_hz=20000000,.trans_queue_depth=1,
        .lcd_cmd_bits=8,.lcd_param_bits=8,.on_color_trans_done=flushed};
    TRY(esp_lcd_new_panel_io_spi((esp_lcd_spi_bus_handle_t)SPI2_HOST,&iocfg,&io));
    esp_lcd_panel_dev_config_t cfg={.reset_gpio_num=21,.rgb_ele_order=LCD_RGB_ELEMENT_ORDER_BGR,.bits_per_pixel=16};
    TRY(esp_lcd_new_panel_st7789(io,&cfg,&panel));
    TRY(esp_lcd_panel_reset(panel)); TRY(esp_lcd_panel_init(panel));
    // ST7789T panel settings from Waveshare's board demo (register values).
    TRY(esp_lcd_panel_io_tx_param(io,0xB0,(uint8_t[]){0x00,0xE8},2));
    TRY(esp_lcd_panel_io_tx_param(io,0xB2,(uint8_t[]){0x0c,0x0c,0x00,0x33,0x33},5));
    TRY(esp_lcd_panel_io_tx_param(io,0xB7,(uint8_t[]){0x75},1));
    TRY(esp_lcd_panel_io_tx_param(io,0xBB,(uint8_t[]){0x1A},1));
    TRY(esp_lcd_panel_io_tx_param(io,0xC0,(uint8_t[]){0x80},1));
    TRY(esp_lcd_panel_io_tx_param(io,0xC2,(uint8_t[]){0x01,0xff},2));
    TRY(esp_lcd_panel_io_tx_param(io,0xC3,(uint8_t[]){0x13},1));
    TRY(esp_lcd_panel_io_tx_param(io,0xC4,(uint8_t[]){0x20},1));
    TRY(esp_lcd_panel_io_tx_param(io,0xC6,(uint8_t[]){0x0f},1));
    TRY(esp_lcd_panel_io_tx_param(io,0xD0,(uint8_t[]){0xA4,0xA1},2));
    TRY(esp_lcd_panel_io_tx_param(io,0xE0,(uint8_t[]){0xD0,0x0D,0x14,0x0D,0x0D,0x09,0x38,0x44,0x4E,0x3A,0x17,0x18,0x2F,0x30},14));
    TRY(esp_lcd_panel_io_tx_param(io,0xE1,(uint8_t[]){0xD0,0x09,0x0F,0x08,0x07,0x14,0x37,0x44,0x4D,0x38,0x15,0x16,0x2C,0x2E},14));
    TRY(esp_lcd_panel_mirror(panel,true,false));
    TRY(esp_lcd_panel_invert_color(panel,true));
    TRY(esp_lcd_panel_set_gap(panel,34,0));
    TRY(esp_lcd_panel_disp_on_off(panel,true));
    ledc_timer_config_t timer={.speed_mode=LEDC_LOW_SPEED_MODE,.duty_resolution=LEDC_TIMER_10_BIT,
        .timer_num=LEDC_TIMER_0,.freq_hz=5000,.clk_cfg=LEDC_AUTO_CLK};
    TRY(ledc_timer_config(&timer));
    ledc_channel_config_t backlight={.gpio_num=22,.speed_mode=LEDC_LOW_SPEED_MODE,
        .channel=LEDC_CHANNEL_0,.timer_sel=LEDC_TIMER_0,.duty=409}; // 40%, Waveshare recommends <=50%.
    TRY(ledc_channel_config(&backlight));
    for(int row=0;row<14;row++) TRY(display_line(row,"",0));
    return ESP_OK;
}
esp_err_t display_line(int row,const char *text,unsigned short color) {
    if(row<0 || row>13) return ESP_ERR_INVALID_ARG;
    int height=row==13?8:24;
    uint16_t bg=0x0821;
    for(int i=0;i<172*height;i++) pixels[i]=(bg<<8)|(bg>>8);
    size_t len=strlen(text); if(len>21) len=21;
    for(size_t c=0;c<len;c++) {
        unsigned ch=(unsigned char)text[c]; if(ch<32 || ch>126) ch='?';
        for(int y=0;y<14 && y+5<height;y++) for(int x=0;x<8;x++) {
            if(font[ch-32][y]&(1<<x)) pixels[(y+5)*172+2+c*8+x]=(color<<8)|(color>>8);
        }
    }
    TRY(esp_lcd_panel_draw_bitmap(panel,0,row*24,172,row*24+height,pixels));
    if(xSemaphoreTake(done,pdMS_TO_TICKS(2000))!=pdTRUE) return ESP_ERR_TIMEOUT;
    return ESP_OK;
}
