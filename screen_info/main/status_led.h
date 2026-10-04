#pragma once
#include <stdbool.h>
#include "esp_err.h"
esp_err_t status_led_init(void);
void status_stage(unsigned stage);
void status_ready(void);
void status_wifi(bool up);
void status_fatal(void);
