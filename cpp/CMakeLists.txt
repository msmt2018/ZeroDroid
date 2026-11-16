cmake_minimum_required(VERSION 3.22)

add_library(zerodroid_vm_core STATIC
    src/vm.cpp
)
target_include_directories(zerodroid_vm_core
    PUBLIC
        ${CMAKE_CURRENT_SOURCE_DIR}/include
)
target_compile_features(zerodroid_vm_core PUBLIC cxx_std_20)
set_target_properties(zerodroid_vm_core PROPERTIES
    POSITION_INDEPENDENT_CODE ON
    CXX_VISIBILITY_PRESET hidden
    VISIBILITY_INLINES_HIDDEN YES
)

if(ZERODROID_BUILD_TESTS AND NOT ANDROID)
    add_executable(zerodroid_vm_core_tests tests/vm_core_tests.cpp)
    target_link_libraries(zerodroid_vm_core_tests PRIVATE zerodroid_vm_core)
    target_compile_features(zerodroid_vm_core_tests PRIVATE cxx_std_20)
    add_test(NAME zerodroid_vm_core_tests COMMAND zerodroid_vm_core_tests)
endif()
