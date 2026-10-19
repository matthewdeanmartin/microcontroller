#pragma once
// The Waveshare ESP32-C6-LCD-1.47's ST7789 (320 x 172 landscape), driven in
// horizontal strips from Rust. Adapted from minicloud_rs/components/display
// (verified on this board); copied, not depended on.
#include <stdint.h>
#include "esp_err.h"

// Initializes the SPI bus, the panel and the backlight. Call once.
esp_err_t gm_display_init(void);
// The DMA-capable strip buffer: 320 x 24 RGB565 pixels, native byte order.
uint16_t *gm_display_buffer(void);
// Sends `rows` rows of the buffer to screen rows top..top+rows; waits for
// the DMA to finish (so the buffer can be redrawn).
esp_err_t gm_display_flush(int top, int rows);
// Backlight 0-100 %. Waveshare recommends at most 50.
esp_err_t gm_display_backlight(int percent);
