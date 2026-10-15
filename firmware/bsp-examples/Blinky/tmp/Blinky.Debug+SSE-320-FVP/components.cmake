# components.cmake

# component ARM::CMSIS Driver:USART@1.1.0
add_library(ARM_CMSIS_Driver_USART_1_1_0 OBJECT
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/CMSIS_Driver/Driver_USART.c"
)
target_include_directories(ARM_CMSIS_Driver_USART_1_1_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/CMSIS_Driver"
  "${CMSIS_PACK_ROOT}/ARM/CMSIS/6.3.0/CMSIS/Driver/Include"
)
target_compile_definitions(ARM_CMSIS_Driver_USART_1_1_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
target_compile_options(ARM_CMSIS_Driver_USART_1_1_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_OPTIONS>
)
target_link_libraries(ARM_CMSIS_Driver_USART_1_1_0 PUBLIC
  ${CONTEXT}_ABSTRACTIONS
)

# component ARM::CMSIS-Compiler:CORE@1.2.1
add_library(ARM_CMSIS-Compiler_CORE_1_2_1 OBJECT
  "${CMSIS_PACK_ROOT}/ARM/CMSIS-Compiler/2.3.0/source/core/gcc/retarget_syscalls.c"
)
target_include_directories(ARM_CMSIS-Compiler_CORE_1_2_1 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
)
target_compile_definitions(ARM_CMSIS-Compiler_CORE_1_2_1 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
target_compile_options(ARM_CMSIS-Compiler_CORE_1_2_1 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_OPTIONS>
)
target_link_libraries(ARM_CMSIS-Compiler_CORE_1_2_1 PUBLIC
  ${CONTEXT}_ABSTRACTIONS
)

# component ARM::CMSIS-Compiler:STDOUT:Custom@1.1.0
add_library(ARM_CMSIS-Compiler_STDOUT_Custom_1_1_0 INTERFACE)
target_include_directories(ARM_CMSIS-Compiler_STDOUT_Custom_1_1_0 INTERFACE
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
  "${CMSIS_PACK_ROOT}/ARM/CMSIS-Compiler/2.3.0/include"
)
target_compile_definitions(ARM_CMSIS-Compiler_STDOUT_Custom_1_1_0 INTERFACE
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
target_link_libraries(ARM_CMSIS-Compiler_STDOUT_Custom_1_1_0 INTERFACE
  ${CONTEXT}_ABSTRACTIONS
)

# component ARM::CMSIS:CORE@6.2.0
add_library(ARM_CMSIS_CORE_6_2_0 INTERFACE)
target_include_directories(ARM_CMSIS_CORE_6_2_0 INTERFACE
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
  "${CMSIS_PACK_ROOT}/ARM/CMSIS/6.3.0/CMSIS/Core/Include"
)
target_compile_definitions(ARM_CMSIS_CORE_6_2_0 INTERFACE
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
target_link_libraries(ARM_CMSIS_CORE_6_2_0 INTERFACE
  ${CONTEXT}_ABSTRACTIONS
)

# component ARM::Device:Definition@2.0.0
add_library(ARM_Device_Definition_2_0_0 OBJECT
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Board/Device_Definition/device_definition.c"
)
target_include_directories(ARM_Device_Definition_2_0_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
  "${SOLUTION_ROOT}/Blinky/RTE/Device/SSE-320-FVP"
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Board/Device_Definition"
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Board/Platform"
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/CMSIS_Driver/Config"
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Include"
)
target_compile_definitions(ARM_Device_Definition_2_0_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
target_compile_options(ARM_Device_Definition_2_0_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_OPTIONS>
)
target_link_libraries(ARM_Device_Definition_2_0_0 PUBLIC
  ${CONTEXT}_ABSTRACTIONS
)

# component ARM::Device:Native Driver:IO@1.0.1
add_library(ARM_Device_Native_Driver_IO_1_0_1 OBJECT
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Source/arm_mps3_io_drv.c"
)
target_include_directories(ARM_Device_Native_Driver_IO_1_0_1 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Include"
)
target_compile_definitions(ARM_Device_Native_Driver_IO_1_0_1 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
target_compile_options(ARM_Device_Native_Driver_IO_1_0_1 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_OPTIONS>
)
target_link_libraries(ARM_Device_Native_Driver_IO_1_0_1 PUBLIC
  ${CONTEXT}_ABSTRACTIONS
)

# component ARM::Device:Native Driver:SysCounter@1.0.1
add_library(ARM_Device_Native_Driver_SysCounter_1_0_1 OBJECT
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Source/syscounter_armv8-m_cntrl_drv.c"
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Source/syscounter_armv8-m_read_drv.c"
)
target_include_directories(ARM_Device_Native_Driver_SysCounter_1_0_1 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Include"
)
target_compile_definitions(ARM_Device_Native_Driver_SysCounter_1_0_1 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
target_compile_options(ARM_Device_Native_Driver_SysCounter_1_0_1 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_OPTIONS>
)
target_link_libraries(ARM_Device_Native_Driver_SysCounter_1_0_1 PUBLIC
  ${CONTEXT}_ABSTRACTIONS
)

# component ARM::Device:Native Driver:SysTimer@1.0.0
add_library(ARM_Device_Native_Driver_SysTimer_1_0_0 OBJECT
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Source/systimer_armv8-m_drv.c"
)
target_include_directories(ARM_Device_Native_Driver_SysTimer_1_0_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Include"
)
target_compile_definitions(ARM_Device_Native_Driver_SysTimer_1_0_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
target_compile_options(ARM_Device_Native_Driver_SysTimer_1_0_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_OPTIONS>
)
target_link_libraries(ARM_Device_Native_Driver_SysTimer_1_0_0 PUBLIC
  ${CONTEXT}_ABSTRACTIONS
)

# component ARM::Device:Native Driver:Timeout@1.0.1
add_library(ARM_Device_Native_Driver_Timeout_1_0_1 OBJECT
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Source/systimer_armv8-m_timeout.c"
)
target_include_directories(ARM_Device_Native_Driver_Timeout_1_0_1 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Include"
)
target_compile_definitions(ARM_Device_Native_Driver_Timeout_1_0_1 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
target_compile_options(ARM_Device_Native_Driver_Timeout_1_0_1 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_OPTIONS>
)
target_link_libraries(ARM_Device_Native_Driver_Timeout_1_0_1 PUBLIC
  ${CONTEXT}_ABSTRACTIONS
)

# component ARM::Device:Native Driver:UART@1.1.0
add_library(ARM_Device_Native_Driver_UART_1_1_0 OBJECT
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Source/uart_cmsdk_drv.c"
)
target_include_directories(ARM_Device_Native_Driver_UART_1_1_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Include"
)
target_compile_definitions(ARM_Device_Native_Driver_UART_1_1_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
target_compile_options(ARM_Device_Native_Driver_UART_1_1_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_OPTIONS>
)
target_link_libraries(ARM_Device_Native_Driver_UART_1_1_0 PUBLIC
  ${CONTEXT}_ABSTRACTIONS
)

# component ARM::Device:Startup&C Startup@2.0.0
add_library(ARM_Device_Startup_C_Startup_2_0_0 OBJECT
  "${SOLUTION_ROOT}/Blinky/RTE/Device/SSE-320-FVP/startup_SSE320.c"
  "${SOLUTION_ROOT}/Blinky/RTE/Device/SSE-320-FVP/system_SSE320.c"
)
target_include_directories(ARM_Device_Startup_C_Startup_2_0_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Board/Platform"
  "${CMSIS_PACK_ROOT}/ARM/SSE_320_BSP/1.1.0/Device/Include"
)
target_compile_definitions(ARM_Device_Startup_C_Startup_2_0_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
target_compile_options(ARM_Device_Startup_C_Startup_2_0_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_OPTIONS>
)
target_link_libraries(ARM_Device_Startup_C_Startup_2_0_0 PUBLIC
  ${CONTEXT}_ABSTRACTIONS
)

# component ARM::Device:USART Retarget@1.0.0
add_library(ARM_Device_USART_Retarget_1_0_0 OBJECT
  "${SOLUTION_ROOT}/Blinky/RTE/Device/SSE-320-FVP/retarget.c"
)
target_include_directories(ARM_Device_USART_Retarget_1_0_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
)
target_compile_definitions(ARM_Device_USART_Retarget_1_0_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
target_compile_options(ARM_Device_USART_Retarget_1_0_0 PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_OPTIONS>
)
target_link_libraries(ARM_Device_USART_Retarget_1_0_0 PUBLIC
  ${CONTEXT}_ABSTRACTIONS
)
