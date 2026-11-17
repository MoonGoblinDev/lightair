# Lightair Effect Stack on Windows (docs/plans/windows.md, phase 3): the ONNX backends
# (tracker/backend-onnx.cpp, tracker/segmenter.cpp) on ONNX Runtime with the DirectML execution
# provider, any DirectX 12 GPU, CPU as fallback. The runtime is loaded at module load from the
# module's data folder (module-main.c), so it can't clash with another plugin's copy.
#
# Output, as OBS lays out modules (what Lightair's engine looks for, runtime.rs):
#   <build>/obs-plugins/64bit/lightair-effect-stack.dll
#   <build>/data/obs-plugins/lightair-effect-stack/  (shaders, presets, models, onnxruntime.dll, DirectML.dll)

set(CMAKE_C_STANDARD 17)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

set(ROOT "${CMAKE_SOURCE_DIR}/../..")
set(OBS_SRC "${ROOT}/third_party/obs-studio")
set(SIMDE_DIR "${ROOT}/third_party/simde")
set(OBS_DIR "$ENV{OBS_DIR}" CACHE PATH "OBS build or install whose obs.lib Lightair links")
if(NOT OBS_DIR)
  set(OBS_DIR "C:/Program Files/obs-studio")
endif()
set(DEPS "${CMAKE_SOURCE_DIR}/.deps")

# Microsoft's NuGet packages: headers, onnxruntime.dll, DirectML.dll.
set(ORT_VERSION 1.20.1)
set(DML_VERSION 1.15.4)
function(nuget name version dir)
  if(NOT EXISTS "${dir}/.done")
    set(pkg "${DEPS}/${name}.${version}.nupkg")
    file(DOWNLOAD "https://www.nuget.org/api/v2/package/${name}/${version}" "${pkg}" STATUS st TLS_VERIFY ON)
    list(GET st 0 code)
    if(NOT code EQUAL 0)
      message(FATAL_ERROR "could not download ${name} ${version}: ${st}")
    endif()
    file(ARCHIVE_EXTRACT INPUT "${pkg}" DESTINATION "${dir}")
    file(TOUCH "${dir}/.done")
  endif()
endfunction()
nuget(Microsoft.ML.OnnxRuntime.DirectML ${ORT_VERSION} "${DEPS}/ort")
nuget(Microsoft.AI.DirectML ${DML_VERSION} "${DEPS}/dml")

set(GEN "${CMAKE_BINARY_DIR}/generated")
file(WRITE "${GEN}/obsconfig.h" "#pragma once\n#define OBS_RELEASE_CANDIDATE 0\n#define OBS_BETA 0\n")

add_library(lightair-effect-stack MODULE
  src/module-main.c
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
  src/tracker/backend-onnx.cpp
  src/tracker/onnx-common.cpp
  src/tracker/segmenter.cpp
)
target_compile_definitions(lightair-effect-stack PRIVATE BEAUTY_HAVE_LIBOBS PLUGIN_VERSION="${PROJECT_VERSION}"
  _USE_MATH_DEFINES NOMINMAX)
# (libobs's threading.h includes <pthread.h>: OBS's own w32-pthreads on Windows)
target_include_directories(lightair-effect-stack PRIVATE "${OBS_SRC}/libobs" "${GEN}" "${SIMDE_DIR}" src src/tracker
  "${OBS_SRC}/deps/w32-pthreads" "${DEPS}/ort/build/native/include")
# Import libraries for libobs and OBS's pthreads: from OBS_DIR when it's a build of OBS
# (lib/<name>.lib); an installed OBS ships only the DLLs, so each library is made from its DLL's
# exports (MSVC's dumpbin and lib).
get_filename_component(MSVC_BIN "${CMAKE_LINKER}" DIRECTORY)
function(obs_implib name out)
  foreach(candidate "${OBS_DIR}/lib/${name}.lib" "${OBS_DIR}/bin/64bit/${name}.lib")
    if(EXISTS "${candidate}")
      set(${out} "${candidate}" PARENT_SCOPE)
      return()
    endif()
  endforeach()
  set(dll "${OBS_DIR}/bin/64bit/${name}.dll")
  if(NOT EXISTS "${dll}")
    message(FATAL_ERROR "no ${name}.lib or ${name}.dll in OBS_DIR (${OBS_DIR})")
  endif()
  find_program(DUMPBIN dumpbin HINTS "${MSVC_BIN}" REQUIRED)
  find_program(MSVC_LIB lib HINTS "${MSVC_BIN}" REQUIRED)
  execute_process(COMMAND "${DUMPBIN}" /nologo /exports "${dll}" OUTPUT_VARIABLE exports COMMAND_ERROR_IS_FATAL ANY)
  string(REGEX MATCHALL "\n +[0-9]+ +[0-9A-F]+ +[0-9A-F]+ +[A-Za-z_][A-Za-z0-9_]*" rows "${exports}")
  set(def "LIBRARY ${name}\nEXPORTS\n")
  foreach(row ${rows})
    string(REGEX REPLACE ".* ([A-Za-z_][A-Za-z0-9_]*)$" "\\1" symbol "${row}")
    string(APPEND def "  ${symbol}\n")
  endforeach()
  file(WRITE "${GEN}/${name}.def" "${def}")
  execute_process(COMMAND "${MSVC_LIB}" /nologo "/def:${GEN}/${name}.def" /machine:x64 "/out:${GEN}/${name}.lib"
    OUTPUT_QUIET COMMAND_ERROR_IS_FATAL ANY)
  set(${out} "${GEN}/${name}.lib" PARENT_SCOPE)
endfunction()
obs_implib(obs OBS_IMPLIB)
obs_implib(w32-pthreads PTHREADS_IMPLIB)
target_link_libraries(lightair-effect-stack PRIVATE "${OBS_IMPLIB}" "${PTHREADS_IMPLIB}" dxgi)
set_target_properties(lightair-effect-stack PROPERTIES PREFIX ""
  LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/obs-plugins/64bit"
  LIBRARY_OUTPUT_DIRECTORY_RELWITHDEBINFO "${CMAKE_BINARY_DIR}/obs-plugins/64bit"
  LIBRARY_OUTPUT_DIRECTORY_RELEASE "${CMAKE_BINARY_DIR}/obs-plugins/64bit")

# The module's data: everything in data/ except the Core ML packages, plus the runtime.
set(DATA_OUT "${CMAKE_BINARY_DIR}/data/obs-plugins/lightair-effect-stack")
add_custom_command(TARGET lightair-effect-stack POST_BUILD
  COMMAND "${CMAKE_COMMAND}" -E copy_directory "${CMAKE_SOURCE_DIR}/data" "${DATA_OUT}"
  COMMAND "${CMAKE_COMMAND}" -E rm -rf "${DATA_OUT}/models/coreml"
  COMMAND "${CMAKE_COMMAND}" -E copy "${DEPS}/ort/runtimes/win-x64/native/onnxruntime.dll" "${DATA_OUT}/"
  COMMAND "${CMAKE_COMMAND}" -E copy "${DEPS}/dml/bin/x64-win/DirectML.dll" "${DATA_OUT}/"
  VERBATIM)
