// Lightair's virtual camera (macOS): a libobs output that sends a canvas to a camera extension,
// so other apps (Zoom, Meet, FaceTime, a browser) can pick Lightair's picture as a camera:
// Lightair's own ("Lightair Virtual Camera", modules/lightair-virtual-camera) when it's
// installed, else OBS's ("OBS Virtual Camera").
//
// OBS's own mac-virtualcam output can't run here: before it sends a frame it asks macOS to
// activate the extension, and that only succeeds from inside OBS.app's bundle. Sending frames
// needs no activation once the extension is installed, so this does only that part: find the
// extension's camera by the device UUID OBS's plugin declares (OBSCameraDeviceUUID in its
// Info.plist), open its sink stream (the device's second stream) and enqueue each frame there.

#import <CoreMediaIO/CMIOHardware.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#include <obs-module.h>
#include <media-io/video-io.h>

struct virtualcam {
	obs_output_t *output;
	CMIODeviceID device;
	CMIOStreamID stream;
	CMSimpleQueueRef queue;
	CVPixelBufferPoolRef pool;
	CMFormatDescriptionRef format;
	uint32_t width, height;
};

static const char *virtualcam_name(void *unused)
{
	UNUSED_PARAMETER(unused);
	return "Lightair virtual camera";
}

static void *virtualcam_create(obs_data_t *settings, obs_output_t *output)
{
	UNUSED_PARAMETER(settings);
	struct virtualcam *v = bzalloc(sizeof(*v));
	v->output = output;
	return v;
}

static void virtualcam_destroy(void *data)
{
	bfree(data);
}

/* Lightair Virtual Camera's device (LightairCameraDeviceUUID in its extension's Info.plist). */
static NSString *const LIGHTAIR_CAMERA_UUID = @"2DF109AC-DE07-42DF-A0D7-098033FFE986";

/* The OBS Virtual Camera's device UUID, from OBS's plugin (next to Lightair's, or in OBS.app). */
static NSString *camera_uuid(void)
{
	NSArray<NSString *> *places = @[
		[[NSBundle mainBundle].builtInPlugInsPath stringByAppendingPathComponent:@"mac-virtualcam.plugin"],
		@"/Applications/OBS.app/Contents/PlugIns/mac-virtualcam.plugin",
	];
	for (NSString *p in places) {
		NSString *uuid = [[NSBundle bundleWithPath:p] objectForInfoDictionaryKey:@"OBSCameraDeviceUUID"];
		if (uuid)
			return uuid;
	}
	return nil;
}

/* The camera with that UUID among CoreMediaIO's devices (0: none, the extension isn't there). */
static CMIODeviceID find_camera(NSString *uuid_string)
{
	CMIOObjectPropertyAddress address = {kCMIOHardwarePropertyDevices, kCMIOObjectPropertyScopeGlobal,
					     kCMIOObjectPropertyElementMain};
	UInt32 size = 0, used = 0;
	if (CMIOObjectGetPropertyDataSize(kCMIOObjectSystemObject, &address, 0, NULL, &size) != noErr || !size)
		return 0;
	NSMutableData *ids = [NSMutableData dataWithLength:size];
	if (CMIOObjectGetPropertyData(kCMIOObjectSystemObject, &address, 0, NULL, size, &used, ids.mutableBytes) !=
	    noErr)
		return 0;
	CFUUIDRef want = CFUUIDCreateFromString(kCFAllocatorDefault, (__bridge CFStringRef)uuid_string);
	CMIODeviceID found = 0;
	const CMIOObjectID *devices = ids.bytes;
	for (size_t i = 0; i < used / sizeof(CMIOObjectID) && !found; i++) {
		CMIOObjectPropertyAddress uid_address = {kCMIODevicePropertyDeviceUID, kCMIOObjectPropertyScopeGlobal,
							 kCMIOObjectPropertyElementMain};
		CFStringRef uid = NULL;
		UInt32 uid_size = sizeof(uid), uid_used = 0;
		if (CMIOObjectGetPropertyData(devices[i], &uid_address, 0, NULL, uid_size, &uid_used, &uid) != noErr ||
		    !uid)
			continue;
		CFUUIDRef have = CFUUIDCreateFromString(kCFAllocatorDefault, uid);
		if (have && want && CFEqual(have, want))
			found = devices[i];
		if (have)
			CFRelease(have);
		CFRelease(uid);
	}
	if (want)
		CFRelease(want);
	return found;
}

/* The sink queue's "it changed" callback: nothing to do (frames are only ever added). */
static void queue_altered(CMIOStreamID stream, void *token, void *refcon)
{
	(void)stream;
	(void)token;
	(void)refcon;
}

static bool virtualcam_start(void *data)
{
	struct virtualcam *v = data;
	@autoreleasepool {
		v->device = find_camera(LIGHTAIR_CAMERA_UUID);
		if (!v->device) {
			NSString *obs = camera_uuid();
			v->device = obs ? find_camera(obs) : 0;
		}
		if (!v->device) {
			obs_output_set_last_error(
				v->output,
				"The virtual camera isn't installed. Open “Lightair Virtual Camera” in Applications to install it.");
			return false;
		}
		CMIOObjectPropertyAddress address = {kCMIODevicePropertyStreams, kCMIOObjectPropertyScopeGlobal,
						     kCMIOObjectPropertyElementMain};
		UInt32 size = 0, used = 0;
		CMIOObjectGetPropertyDataSize(v->device, &address, 0, NULL, &size);
		if (size < 2 * sizeof(CMIOStreamID)) {
			obs_output_set_last_error(v->output, "The virtual camera isn't ready. Try again in a moment.");
			return false;
		}
		NSMutableData *streams = [NSMutableData dataWithLength:size];
		CMIOObjectGetPropertyData(v->device, &address, 0, NULL, size, &used, streams.mutableBytes);
		/* the second stream takes frames in (the first is what apps read) */
		v->stream = ((const CMIOStreamID *)streams.bytes)[1];

		video_t *video = obs_output_video(v->output);
		const struct video_output_info *info = video ? video_output_get_info(video) : NULL;
		if (!info) {
			obs_output_set_last_error(v->output, "No picture to send.");
			return false;
		}
		v->width = info->width;
		v->height = info->height;
		/* NV12, which Core Video takes as it is */
		struct video_scale_info conversion = {
			.format = VIDEO_FORMAT_NV12,
			.width = info->width,
			.height = info->height,
			.range = info->range,
			.colorspace = info->colorspace,
		};
		obs_output_set_video_conversion(v->output, &conversion);
		OSType pixel = info->range == VIDEO_RANGE_FULL ? kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
							       : kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
		NSDictionary *attrs = @{
			(id)kCVPixelBufferPixelFormatTypeKey: @(pixel),
			(id)kCVPixelBufferWidthKey: @(info->width),
			(id)kCVPixelBufferHeightKey: @(info->height),
			(id)kCVPixelBufferIOSurfacePropertiesKey: @{},
		};
		if (CVPixelBufferPoolCreate(kCFAllocatorDefault, NULL, (__bridge CFDictionaryRef)attrs, &v->pool) !=
		    kCVReturnSuccess) {
			obs_output_set_last_error(v->output, "Couldn't make frames for the virtual camera.");
			return false;
		}
		CMVideoFormatDescriptionCreate(kCFAllocatorDefault, pixel, (int32_t)info->width, (int32_t)info->height,
					       NULL, &v->format);
		OSStatus queued = CMIOStreamCopyBufferQueue(v->stream, queue_altered, NULL, &v->queue);
		OSStatus started = queued == noErr && v->queue ? CMIODeviceStartStream(v->device, v->stream) : -1;
		if (queued != noErr || !v->queue || started != noErr) {
			blog(LOG_WARNING, "[virtual camera] sink stream %u: queue %d, start %d", (unsigned)v->stream,
			     (int)queued, (int)started);
			obs_output_set_last_error(v->output, "The virtual camera didn't start. Try again in a moment.");
			if (v->queue) {
				CFRelease(v->queue);
				v->queue = NULL;
			}
			CFRelease(v->format);
			v->format = NULL;
			CVPixelBufferPoolRelease(v->pool);
			v->pool = NULL;
			return false;
		}
	}
	return obs_output_begin_data_capture(v->output, 0);
}

static void virtualcam_stop(void *data, uint64_t ts)
{
	UNUSED_PARAMETER(ts);
	struct virtualcam *v = data;
	obs_output_end_data_capture(v->output);
	if (v->device && v->stream)
		CMIODeviceStopStream(v->device, v->stream);
	if (v->queue) {
		CFRelease(v->queue);
		v->queue = NULL;
	}
	if (v->format) {
		CFRelease(v->format);
		v->format = NULL;
	}
	if (v->pool) {
		CVPixelBufferPoolRelease(v->pool);
		v->pool = NULL;
	}
}

static void virtualcam_video(void *data, struct video_data *frame)
{
	struct virtualcam *v = data;
	if (!v->pool || !v->queue)
		return;
	CVPixelBufferRef buffer = NULL;
	if (CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, v->pool, &buffer) != kCVReturnSuccess)
		return;
	CVPixelBufferLockBaseAddress(buffer, 0);
	for (size_t plane = 0; plane < 2; plane++) {
		const uint8_t *src = frame->data[plane];
		uint8_t *dst = CVPixelBufferGetBaseAddressOfPlane(buffer, plane);
		if (!src || !dst)
			break;
		size_t dst_row = CVPixelBufferGetBytesPerRowOfPlane(buffer, plane);
		size_t src_row = frame->linesize[plane];
		size_t rows = CVPixelBufferGetHeightOfPlane(buffer, plane);
		size_t n = src_row < dst_row ? src_row : dst_row;
		for (size_t r = 0; r < rows; r++)
			memcpy(dst + r * dst_row, src + r * src_row, n);
	}
	CVPixelBufferUnlockBaseAddress(buffer, 0);
	CMSampleTimingInfo timing = {.duration = kCMTimeInvalid,
				     .presentationTimeStamp = CMTimeMake((int64_t)frame->timestamp, 1000000000),
				     .decodeTimeStamp = kCMTimeInvalid};
	CMSampleBufferRef sample = NULL;
	if (CMSampleBufferCreateForImageBuffer(kCFAllocatorDefault, buffer, true, NULL, NULL, v->format, &timing,
					       &sample) == noErr &&
	    sample) {
		/* a full queue (no app reading): the frame is dropped */
		if (CMSimpleQueueEnqueue(v->queue, sample) != noErr)
			CFRelease(sample);
	}
	CVPixelBufferRelease(buffer);
}

struct obs_output_info lightair_virtualcam_info = {
	.id = "lightair_virtualcam",
	.flags = OBS_OUTPUT_VIDEO,
	.get_name = virtualcam_name,
	.create = virtualcam_create,
	.destroy = virtualcam_destroy,
	.start = virtualcam_start,
	.stop = virtualcam_stop,
	.raw_video = virtualcam_video,
};
