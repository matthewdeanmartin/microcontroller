#pragma once
#include "esp_err.h"
esp_err_t display_init(void);
esp_err_t display_line(int row, const char *text, unsigned short color);
