// ST7789 strip driver for Game Matcher. Panel settings and pins from
// minicloud_rs/components/display/display.c (Waveshare's demo values,
// verified on this board, ac:eb:e6:1e:13:40).
#include "gm_display.h"
#include "driver/spi_master.h"
#include "driver/gpio.h"
#include "driver/ledc.h"
#include "esp_lcd_panel_io.h"
#include "esp_lcd_panel_ops.h"
#include "esp_lcd_panel_vendor.h"
#include "esp_heap_caps.h"
#include "freertos/FreeRTOS.h"
#include "freertos/semphr.h"

#define WIDTH 320
#define HEIGHT 172
#define STRIP 24

static esp_lcd_panel_handle_t panel;
static uint16_t *pixels;
static SemaphoreHandle_t done;

static bool flushed(esp_lcd_panel_io_handle_t io, esp_lcd_panel_io_event_data_t *event, void *ctx) {
    BaseType_t wake = pdFALSE;
    xSemaphoreGiveFromISR(done, &wake);
    return wake == pdTRUE;
}

#define TRY(x) do { esp_err_t e = (x); if (e) return e; } while (0)

esp_err_t gm_display_init(void) {
    done = xSemaphoreCreateBinary();
    pixels = heap_caps_calloc(WIDTH * STRIP, sizeof(*pixels), MALLOC_CAP_DMA);
    if (!done || !pixels) return ESP_ERR_NO_MEM;
    // The TF card shares this SPI bus: deselect it (CS GPIO4) before any
    // LCD traffic, as minicloud does.
    TRY(gpio_set_direction(4, GPIO_MODE_OUTPUT)); TRY(gpio_set_level(4, 1));
    TRY(gpio_set_direction(14, GPIO_MODE_OUTPUT)); TRY(gpio_set_level(14, 1));
    spi_bus_config_t bus = {.mosi_io_num = 6, .miso_io_num = 5, .sclk_io_num = 7,
        .quadwp_io_num = -1, .quadhd_io_num = -1, .max_transfer_sz = WIDTH * STRIP * 2};
    TRY(spi_bus_initialize(SPI2_HOST, &bus, SPI_DMA_CH_AUTO));
    esp_lcd_panel_io_handle_t io;
    esp_lcd_panel_io_spi_config_t iocfg = {.cs_gpio_num = 14, .dc_gpio_num = 15,
        .spi_mode = 0, .pclk_hz = 20000000, .trans_queue_depth = 1,
        .lcd_cmd_bits = 8, .lcd_param_bits = 8, .on_color_trans_done = flushed};
    TRY(esp_lcd_new_panel_io_spi((esp_lcd_spi_bus_handle_t)SPI2_HOST, &iocfg, &io));
    esp_lcd_panel_dev_config_t cfg = {.reset_gpio_num = 21,
        .rgb_ele_order = LCD_RGB_ELEMENT_ORDER_BGR, .bits_per_pixel = 16};
    TRY(esp_lcd_new_panel_st7789(io, &cfg, &panel));
    TRY(esp_lcd_panel_reset(panel));
    TRY(esp_lcd_panel_init(panel));
    // ST7789T settings from Waveshare's board demo (register values).
    TRY(esp_lcd_panel_io_tx_param(io, 0xB0, (uint8_t[]){0x00, 0xE8}, 2));
    TRY(esp_lcd_panel_io_tx_param(io, 0xB2, (uint8_t[]){0x0c, 0x0c, 0x00, 0x33, 0x33}, 5));
    TRY(esp_lcd_panel_io_tx_param(io, 0xB7, (uint8_t[]){0x75}, 1));
    TRY(esp_lcd_panel_io_tx_param(io, 0xBB, (uint8_t[]){0x1A}, 1));
    TRY(esp_lcd_panel_io_tx_param(io, 0xC0, (uint8_t[]){0x80}, 1));
    TRY(esp_lcd_panel_io_tx_param(io, 0xC2, (uint8_t[]){0x01, 0xff}, 2));
    TRY(esp_lcd_panel_io_tx_param(io, 0xC3, (uint8_t[]){0x13}, 1));
    TRY(esp_lcd_panel_io_tx_param(io, 0xC4, (uint8_t[]){0x20}, 1));
    TRY(esp_lcd_panel_io_tx_param(io, 0xC6, (uint8_t[]){0x0f}, 1));
    TRY(esp_lcd_panel_io_tx_param(io, 0xD0, (uint8_t[]){0xA4, 0xA1}, 2));
    TRY(esp_lcd_panel_io_tx_param(io, 0xE0, (uint8_t[]){0xD0, 0x0D, 0x14, 0x0D, 0x0D, 0x09, 0x38, 0x44, 0x4E, 0x3A, 0x17, 0x18, 0x2F, 0x30}, 14));
    TRY(esp_lcd_panel_io_tx_param(io, 0xE1, (uint8_t[]){0xD0, 0x09, 0x0F, 0x08, 0x07, 0x14, 0x37, 0x44, 0x4D, 0x38, 0x15, 0x16, 0x2C, 0x2E}, 14));
    TRY(esp_lcd_panel_swap_xy(panel, true));
    TRY(esp_lcd_panel_mirror(panel, true, true));
    TRY(esp_lcd_panel_invert_color(panel, true));
    TRY(esp_lcd_panel_set_gap(panel, 0, 34));
    TRY(esp_lcd_panel_disp_on_off(panel, true));
    ledc_timer_config_t timer = {.speed_mode = LEDC_LOW_SPEED_MODE, .duty_resolution = LEDC_TIMER_10_BIT,
        .timer_num = LEDC_TIMER_0, .freq_hz = 5000, .clk_cfg = LEDC_AUTO_CLK};
    TRY(ledc_timer_config(&timer));
    ledc_channel_config_t backlight = {.gpio_num = 22, .speed_mode = LEDC_LOW_SPEED_MODE,
        .channel = LEDC_CHANNEL_0, .timer_sel = LEDC_TIMER_0, .duty = 409};  // 40 %
    TRY(ledc_channel_config(&backlight));
    return ESP_OK;
}

uint16_t *gm_display_buffer(void) { return pixels; }

esp_err_t gm_display_flush(int top, int rows) {
    if (!panel || top < 0 || rows < 1 || rows > STRIP || top + rows > HEIGHT) return ESP_ERR_INVALID_ARG;
    TRY(esp_lcd_panel_draw_bitmap(panel, 0, top, WIDTH, top + rows, pixels));
    if (xSemaphoreTake(done, pdMS_TO_TICKS(2000)) != pdTRUE) return ESP_ERR_TIMEOUT;
    return ESP_OK;
}

esp_err_t gm_display_backlight(int percent) {
    if (percent < 0) percent = 0;
    if (percent > 100) percent = 100;
    TRY(ledc_set_duty(LEDC_LOW_SPEED_MODE, LEDC_CHANNEL_0, percent * 1023 / 100));
    return ledc_update_duty(LEDC_LOW_SPEED_MODE, LEDC_CHANNEL_0);
}
