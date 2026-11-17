/*
Lightair Effect Stack: face-aware camera pipeline (docs/09-effect-stack.md).
Built from the Dewy beauty pipeline. Copyright (C) 2026 Bregas S. Wicaksono.

This program is free software; you can redistribute it and/or modify it under the terms of the
GNU General Public License as published by the Free Software Foundation; either version 3 of
the License, or (at your option) any later version.
*/

#include <obs-module.h>

#include "makeup.h"
#include "presets.h"
#include "face-tracker.h"

OBS_DECLARE_MODULE()
OBS_MODULE_USE_DEFAULT_LOCALE("lightair-effect-stack", "en-US")

extern struct obs_source_info beauty_filter_info;

const char *obs_module_name(void)
{
	return "Lightair Effect Stack";
}

const char *obs_module_description(void)
{
	return "Face-aware skin, shape, makeup, effects, atmosphere, colour and light for cameras.";
}

extern struct obs_source_info shape_mask_info;
extern struct obs_source_info transform3d_info;
extern struct obs_source_info fx_filter_info;
#ifdef __APPLE__
extern struct obs_output_info lightair_virtualcam_info;
#endif

bool obs_module_load(void)
{
	makeup_presets_load();
	presets_load();
#ifdef _WIN32
	/* ONNX Runtime (with DirectML) ships in the module's data folder */
	char *ort = obs_module_file("onnxruntime.dll");
	face_tracker_set_runtime_library(ort);
	bfree(ort);
#endif
	obs_register_source(&beauty_filter_info);
	obs_register_source(&shape_mask_info);
	obs_register_source(&transform3d_info);
	obs_register_source(&fx_filter_info);
#ifdef __APPLE__
	obs_register_output(&lightair_virtualcam_info);
#endif
	blog(LOG_INFO, "[effect-stack] loaded");
	return true;
}

void obs_module_unload(void)
{
	makeup_presets_free();
	presets_free();
}
