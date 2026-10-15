cmake_minimum_required(VERSION 3.27)

# Roots
include("../roots.cmake")

set(CONTEXT Vio.Debug+SSE-320-FVP)
set(TARGET ${CONTEXT})
set(DNAME SSE-320-FVP)
set(DPACK ARM::SSE_320_BSP@1.1.0)
set(DPACK_DIR "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0")
set(OUT_DIR "${SOLUTION_ROOT}/build/SSE-320-FVP/GCC/Debug/Vio/outdir")
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
set(CMAKE_COMPILE_COMMANDS ${CMAKE_CURRENT_BINARY_DIR}/compile_commands.json)
set(COMPILE_COMMANDS ${OUT_DIR}/compile_commands.json)
set(COMPILE_MACROS_C ${OUT_DIR}/compile_macros_c.h)
set(LD_SCRIPT "${SOLUTION_ROOT}/Vio/RTE/Device/SSE-320-FVP/gcc_linker_script.ld.src")
set(LD_REGIONS "${SOLUTION_ROOT}/Vio/RTE/Device/SSE-320-FVP/regions_SSE-320.h")
set(LD_SCRIPT_PP "${CMAKE_CURRENT_BINARY_DIR}/gcc_linker_script.ld")

# Processor Options
set(CPU Cortex-M85)
set(FPU DP_FPU)
set(DSP DSP)
set(MVE FP_MVE)

# Preprocessor options
set(CPP_OPTIONS_C "-xc" "-std=gnu11" "-masm-syntax-unified" "-fomit-frame-pointer")

# Toolchain config map
set(COMPILER GCC)
include("toolchain.cmake")

# Setup project
project(${CONTEXT} LANGUAGES C)

# Enable color diagnostics
set(CMAKE_COLOR_DIAGNOSTICS ON)

# Compilation database
add_custom_target(database DEPENDS ${COMPILE_COMMANDS} ${COMPILE_MACROS_C})
add_custom_command(OUTPUT ${COMPILE_COMMANDS}
  COMMAND ${CMAKE_COMMAND} -E copy_if_different "${CMAKE_COMPILE_COMMANDS}" "${COMPILE_COMMANDS}"
  DEPENDS "${CMAKE_COMPILE_COMMANDS}"
)
add_custom_command(OUTPUT ${COMPILE_MACROS_C}
  COMMAND ${CPP} ${CPP_OPTIONS_C} ${CPP_DUMP_MACROS} "${COMPILE_MACROS_C}"
)
set(CMAKE_C_STANDARD_INCLUDE_DIRECTORIES ${CMAKE_C_IMPLICIT_INCLUDE_DIRECTORIES})

# Setup context
add_executable(${CONTEXT})
set_target_properties(${CONTEXT} PROPERTIES PREFIX "" SUFFIX ".elf" OUTPUT_NAME "Vio")
set_target_properties(${CONTEXT} PROPERTIES RUNTIME_OUTPUT_DIRECTORY ${OUT_DIR})
add_library(${CONTEXT}_GLOBAL INTERFACE)

# Includes
target_include_directories(${CONTEXT} PUBLIC
  "${SOLUTION_ROOT}/Vio/RTE/_Debug_SSE-320-FVP"
  "${SOLUTION_ROOT}/Vio/RTE/Device/SSE-320-FVP"
  "${CMSIS_PACK_ROOT}/ARM/CMSIS-Compiler/2.3.0/include"
  "${CMSIS_PACK_ROOT}/ARM/CMSIS/6.3.0/CMSIS/Core/Include"
  "${CMSIS_PACK_ROOT}/ARM/CMSIS/6.3.0/CMSIS/Driver/Include"
  "${CMSIS_PACK_ROOT}/ARM/CMSIS/6.3.0/CMSIS/Driver/VIO/Include"
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Board/Device_Definition"
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Board/Platform"
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/CMSIS_Driver"
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/CMSIS_Driver/Config"
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Include"
)

# Defines
target_compile_definitions(${CONTEXT} PUBLIC
  $<$<COMPILE_LANGUAGE:C,CXX>:
    _RTE_
  >
)

# Compile options
target_compile_options(${CONTEXT} PUBLIC
  $<$<COMPILE_LANGUAGE:C>:
    "SHELL:${CC_CPU}"
    "SHELL:${CC_FLAGS}"
    "SHELL:-std=gnu11"
    "SHELL:-masm-syntax-unified"
    "SHELL:-fomit-frame-pointer"
  >
)

# Compile Options Abstractions
add_library(${CONTEXT}_ABSTRACTIONS INTERFACE)
cbuild_set_options_flags(CC "none" "on" "" "" CC_OPTIONS_FLAGS_${CONTEXT})
target_compile_options(${CONTEXT}_ABSTRACTIONS INTERFACE
  $<$<COMPILE_LANGUAGE:C>:
    "SHELL:${CC_OPTIONS_FLAGS_${CONTEXT}}"
  >
)

# Add groups and components
include("groups.cmake")
include("components.cmake")

target_link_libraries(${CONTEXT} PUBLIC
  Group_Main
  ARM_CMSIS_Driver_USART_1_1_0
  ARM_CMSIS_Driver_VIO_1_0_0
  ARM_CMSIS-Compiler_CORE_1_2_1
  ARM_CMSIS-Compiler_STDOUT_Custom_1_1_0
  ARM_CMSIS_CORE_6_2_0
  ARM_Device_Definition_2_0_0
  ARM_Device_Native_Driver_SysCounter_1_0_1
  ARM_Device_Native_Driver_SysTimer_1_0_0
  ARM_Device_Native_Driver_Timeout_1_0_1
  ARM_Device_Native_Driver_UART_1_1_0
  ARM_Device_Startup_C_Startup_2_0_0
  ARM_Device_USART_Retarget_1_0_0
)

# Linker options
target_link_options(${CONTEXT} PUBLIC
  "SHELL:${LD_CPU}"
  "SHELL:${_LS}\"${LD_SCRIPT_PP}\""
  "SHELL:--specs=nano.specs"
  "SHELL:-Wl,-Map=${SOLUTION_ROOT}/build/SSE-320-FVP/GCC/Debug/Vio/outdir/Vio.elf.map"
)
set(LD_DEPS ${LD_SCRIPT} ${LD_REGIONS})
set_target_properties(${CONTEXT} PROPERTIES LINK_DEPENDS "${LD_DEPS}")

# Linker script pre-processing
add_custom_command(TARGET ${CONTEXT} PRE_LINK COMMAND ${CPP} ARGS ${CPP_ARGS_LD_SCRIPT} BYPRODUCTS ${LD_SCRIPT_PP})
