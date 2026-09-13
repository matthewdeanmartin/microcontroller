# Top-level project file. Must come before the project() call.
cmake_minimum_required(VERSION 3.16)

# Pulls in the whole ESP-IDF build system.
include($ENV{IDF_PATH}/tools/cmake/project.cmake)

# The project name becomes the .bin/.elf filename.
project(hello_wifi)
