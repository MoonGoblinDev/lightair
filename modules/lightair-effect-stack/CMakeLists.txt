# Lightair Effect Stack (docs/09-effect-stack.md): a libobs module built in-tree.
# macOS: lightair-effect-stack.plugin, tracking and segmentation on Core ML, linked against an
# installed OBS 33's libobs.framework (OBS_APP), like the rest of Lightair (IMPLEMENTATION.md).
# Windows: lightair-effect-stack.dll, the same on ONNX Runtime with DirectML, linked against
# OBS_DIR's obs.lib, laid out as OBS lays out its modules (windows.cmake).
cmake_minimum_required(VERSION 3.24)
# (WIN32 is only known after project(); the host is: Lightair builds natively on Windows)
if(CMAKE_HOST_WIN32)
  project(lightair-effect-stack VERSION 0.1.0 LANGUAGES C CXX)
  include(cmake/windows.cmake)
  return()
endif()
project(lightair-effect-stack VERSION 0.1.0 LANGUAGES C CXX OBJC OBJCXX)

set(CMAKE_C_STANDARD 17)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_OSX_DEPLOYMENT_TARGET "14.0" CACHE STRING "")

set(ROOT "${CMAKE_SOURCE_DIR}/../..")
set(OBS_SRC "${ROOT}/third_party/obs-studio")
set(SIMDE_DIR "${ROOT}/third_party/simde")
set(OBS_APP "/Applications/OBS.app" CACHE PATH "Installed OBS.app to link against in development")
set(DEPS "${CMAKE_SOURCE_DIR}/.deps")

# Face tracking and segmentation run on Core ML: MediaPipe's face landmarker (478-point mesh),
# YuNet detection and the multiclass selfie segmenter, converted to .mlpackage
# (data/models/coreml, made by tools/convert-coreml.py) and compiled into the bundle below.

set(GEN "${CMAKE_BINARY_DIR}/generated")
file(WRITE "${GEN}/obsconfig.h" "#pragma once\n#define OBS_RELEASE_CANDIDATE 0\n#define OBS_BETA 0\n")

add_library(lightair-effect-stack MODULE
  src/module-main.c
  src/face-api.c
  src/beauty-filter.c
  src/face-geometry.c
  src/makeup.c
  src/grade.c
  src/presets.c
  src/fx-layers.c
  src/shape-mask.c
  src/transform3d.c
  src/fx-filter.c
  src/tracker/tracker-core.cpp
  src/tracker/tracker-util.cpp
  src/tracker/pigment-calib.cpp
  src/tracker/mediapipe.cpp
  src/tracker/backend-coreml.mm
  src/tracker/coreml-common.mm
  src/tracker/segmenter-coreml.mm
  src/tracker/person-vision.mm
  src/virtualcam.m
  src/tracker/body-mediapipe.cpp
  src/tracker/body-tracker.cpp
  src/tracker/body-coreml.mm
)
set_source_files_properties(src/tracker/backend-coreml.mm src/tracker/coreml-common.mm src/tracker/segmenter-coreml.mm
  src/tracker/person-vision.mm src/virtualcam.m src/tracker/body-coreml.mm
  PROPERTIES COMPILE_FLAGS "-fobjc-arc")
target_compile_definitions(lightair-effect-stack PRIVATE BEAUTY_TRACKER_COREML BEAUTY_HAVE_LIBOBS PLUGIN_VERSION="${PROJECT_VERSION}")
target_include_directories(lightair-effect-stack PRIVATE "${OBS_SRC}/libobs" "${GEN}" "${SIMDE_DIR}" src src/tracker)
target_compile_options(lightair-effect-stack PRIVATE -Wall -Wno-unused-parameter -Wno-missing-field-initializers)
target_link_options(lightair-effect-stack PRIVATE "-F${OBS_APP}/Contents/Frameworks" "-Wl,-rpath,@executable_path/../Frameworks")
target_link_libraries(lightair-effect-stack PRIVATE "-framework libobs" "-framework Foundation" "-framework CoreVideo"
  "-framework CoreGraphics" "-framework Vision" "-framework CoreML" "-framework Accelerate"
  "-framework CoreMedia" "-framework CoreMediaIO")

file(GLOB_RECURSE MODULE_DATA RELATIVE "${CMAKE_SOURCE_DIR}/data" CONFIGURE_DEPENDS "${CMAKE_SOURCE_DIR}/data/*")
list(FILTER MODULE_DATA EXCLUDE REGEX "^models/")
foreach(f IN LISTS MODULE_DATA)
  get_filename_component(dir "${f}" DIRECTORY)
  target_sources(lightair-effect-stack PRIVATE "data/${f}")
  set_source_files_properties("data/${f}" PROPERTIES MACOSX_PACKAGE_LOCATION "Resources/${dir}")
endforeach()
# Core ML models, compiled to .mlmodelc and copied into Resources/models.
set(COREML_OUT "${CMAKE_BINARY_DIR}/coreml")
set(COREML_COMPILED "")
foreach(m selfie_multiclass_256 face_detection_yunet face_landmarks_478
    person_detection_mediapipe_2023mar pose_estimation_mediapipe_2023mar handpose_estimation_mediapipe_2023feb)
  set(pkg "${CMAKE_SOURCE_DIR}/data/models/coreml/${m}.mlpackage")
  if(NOT EXISTS "${pkg}")
    message(FATAL_ERROR "missing ${pkg}: run tools/convert-coreml.py")
  endif()
  file(GLOB_RECURSE pkg_files "${pkg}/*")
  add_custom_command(OUTPUT "${COREML_OUT}/${m}.mlmodelc/coremldata.bin"
    COMMAND "${CMAKE_COMMAND}" -E rm -rf "${COREML_OUT}/${m}.mlmodelc"
    COMMAND xcrun coremlcompiler compile "${pkg}" "${COREML_OUT}" > /dev/null
    DEPENDS ${pkg_files} VERBATIM)
  list(APPEND COREML_COMPILED "${COREML_OUT}/${m}.mlmodelc/coremldata.bin")
  add_custom_command(TARGET lightair-effect-stack POST_BUILD
    COMMAND "${CMAKE_COMMAND}" -E rm -rf "$<TARGET_BUNDLE_CONTENT_DIR:lightair-effect-stack>/Resources/models/${m}.mlmodelc"
    COMMAND "${CMAKE_COMMAND}" -E copy_directory "${COREML_OUT}/${m}.mlmodelc"
            "$<TARGET_BUNDLE_CONTENT_DIR:lightair-effect-stack>/Resources/models/${m}.mlmodelc")
endforeach()
add_custom_target(coreml-models DEPENDS ${COREML_COMPILED})
add_dependencies(lightair-effect-stack coreml-models)
set_target_properties(lightair-effect-stack PROPERTIES
  BUNDLE TRUE BUNDLE_EXTENSION plugin PREFIX ""
  MACOSX_BUNDLE_INFO_PLIST "${CMAKE_SOURCE_DIR}/cmake/Info.plist.in"
  MACOSX_BUNDLE_GUI_IDENTIFIER "app.lightair.effect-stack"
  MACOSX_BUNDLE_BUNDLE_VERSION "${PROJECT_VERSION}"
  MACOSX_BUNDLE_SHORT_VERSION_STRING "${PROJECT_VERSION}")
