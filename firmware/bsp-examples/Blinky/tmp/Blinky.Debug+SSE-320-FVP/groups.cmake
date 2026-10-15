# groups.cmake

# group Main
add_library(Group_Main OBJECT
  "${SOLUTION_ROOT}/Blinky/main.c"
  "${SOLUTION_ROOT}/Blinky/Blinky.c"
)
target_include_directories(Group_Main PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
)
target_compile_definitions(Group_Main PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
add_library(Group_Main_ABSTRACTIONS INTERFACE)
target_link_libraries(Group_Main_ABSTRACTIONS INTERFACE
  ${CONTEXT}_ABSTRACTIONS
)
target_compile_options(Group_Main PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_OPTIONS>
)
target_link_libraries(Group_Main PUBLIC
  Group_Main_ABSTRACTIONS
)

# group API
add_library(Group_API OBJECT
  "${SOLUTION_ROOT}/Blinky/API/Source/led_port.c"
)
target_include_directories(Group_API PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_INCLUDE_DIRECTORIES>
  "${SOLUTION_ROOT}/Blinky/API/Include"
)
target_compile_definitions(Group_API PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_DEFINITIONS>
)
add_library(Group_API_ABSTRACTIONS INTERFACE)
target_link_libraries(Group_API_ABSTRACTIONS INTERFACE
  ${CONTEXT}_ABSTRACTIONS
)
target_compile_options(Group_API PUBLIC
  $<TARGET_PROPERTY:${CONTEXT},INTERFACE_COMPILE_OPTIONS>
)
target_link_libraries(Group_API PUBLIC
  Group_API_ABSTRACTIONS
)
