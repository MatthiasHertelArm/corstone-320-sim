# groups.cmake

# group Main
add_library(Group_Main OBJECT
  "${SOLUTION_ROOT}/Vio/main.c"
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
