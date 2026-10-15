# Distributed under the OSI-approved BSD 3-Clause License.  See accompanying
# file LICENSE.rst or https://cmake.org/licensing for details.

cmake_minimum_required(VERSION ${CMAKE_VERSION}) # this file comes with cmake

# If CMAKE_DISABLE_SOURCE_CHANGES is set to true and the source directory is an
# existing directory in our source tree, calling file(MAKE_DIRECTORY) on it
# would cause a fatal error, even though it would be a no-op.
if(NOT EXISTS "/Users/mather01/Projects/corstone-320-sim/firmware/bsp-examples/Blinky/tmp/Blinky.Debug+SSE-320-FVP")
  file(MAKE_DIRECTORY "/Users/mather01/Projects/corstone-320-sim/firmware/bsp-examples/Blinky/tmp/Blinky.Debug+SSE-320-FVP")
endif()
file(MAKE_DIRECTORY
  "/Users/mather01/Projects/corstone-320-sim/firmware/bsp-examples/Blinky/tmp/1"
  "/Users/mather01/Projects/corstone-320-sim/firmware/bsp-examples/Blinky/tmp/Blinky.Debug+SSE-320-FVP"
  "/Users/mather01/Projects/corstone-320-sim/firmware/bsp-examples/Blinky/tmp/Blinky.Debug+SSE-320-FVP/tmp"
  "/Users/mather01/Projects/corstone-320-sim/firmware/bsp-examples/Blinky/tmp/Blinky.Debug+SSE-320-FVP/src/Blinky.Debug+SSE-320-FVP-stamp"
  "/Users/mather01/Projects/corstone-320-sim/firmware/bsp-examples/Blinky/tmp/Blinky.Debug+SSE-320-FVP/src"
  "/Users/mather01/Projects/corstone-320-sim/firmware/bsp-examples/Blinky/tmp/Blinky.Debug+SSE-320-FVP/src/Blinky.Debug+SSE-320-FVP-stamp"
)

set(configSubDirs )
foreach(subDir IN LISTS configSubDirs)
    file(MAKE_DIRECTORY "/Users/mather01/Projects/corstone-320-sim/firmware/bsp-examples/Blinky/tmp/Blinky.Debug+SSE-320-FVP/src/Blinky.Debug+SSE-320-FVP-stamp/${subDir}")
endforeach()
if(cfgdir)
  file(MAKE_DIRECTORY "/Users/mather01/Projects/corstone-320-sim/firmware/bsp-examples/Blinky/tmp/Blinky.Debug+SSE-320-FVP/src/Blinky.Debug+SSE-320-FVP-stamp${cfgdir}") # cfgdir has leading slash
endif()
