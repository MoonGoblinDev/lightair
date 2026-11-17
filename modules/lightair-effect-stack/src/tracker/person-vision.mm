// Apple Vision person segmentation (VNGeneratePersonSegmentationRequest) at the balanced quality
// level asked for (balanced is what Apple recommends for video), through a sequence handler so the mask stays steady from frame
// to frame.

#include "person-vision.hpp"

#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#import <Vision/Vision.h>

#include <cstring>

struct PersonVision::Impl {
	VNGeneratePersonSegmentationRequest *request = nil;
	VNSequenceRequestHandler *sequence = nil;
	CVPixelBufferPoolRef pool = nullptr;
	int pool_w = 0, pool_h = 0;
	int quality = -1;

	void configure(int q)
	{
		@autoreleasepool {
			request = [VNGeneratePersonSegmentationRequest new];
			request.qualityLevel = q == 0   ? VNGeneratePersonSegmentationRequestQualityLevelFast
					       : q == 2 ? VNGeneratePersonSegmentationRequestQualityLevelAccurate
							: VNGeneratePersonSegmentationRequestQualityLevelBalanced;
			request.outputPixelFormat = kCVPixelFormatType_OneComponent8;
			sequence = [VNSequenceRequestHandler new]; /* a fresh sequence for the new level */
			quality = q;
		}
	}

	CVPixelBufferRef wrap(const uint8_t *bgra, int w, int h, int stride)
	{
		if (!pool || pool_w != w || pool_h != h) {
			if (pool)
				CVPixelBufferPoolRelease(pool);
			pool = nullptr;
			NSDictionary *attrs = @{
				(id)kCVPixelBufferPixelFormatTypeKey: @(kCVPixelFormatType_32BGRA),
				(id)kCVPixelBufferWidthKey: @(w),
				(id)kCVPixelBufferHeightKey: @(h),
				(id)kCVPixelBufferIOSurfacePropertiesKey: @{},
			};
			CVPixelBufferPoolCreate(kCFAllocatorDefault, nullptr, (__bridge CFDictionaryRef)attrs, &pool);
			pool_w = w;
			pool_h = h;
		}
		CVPixelBufferRef buf = nullptr;
		if (!pool || CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, pool, &buf) != kCVReturnSuccess)
			return nullptr;
		CVPixelBufferLockBaseAddress(buf, 0);
		uint8_t *dst = (uint8_t *)CVPixelBufferGetBaseAddress(buf);
		size_t ds = CVPixelBufferGetBytesPerRow(buf);
		for (int y = 0; y < h; y++)
			memcpy(dst + y * ds, bgra + (size_t)y * stride, (size_t)w * 4);
		CVPixelBufferUnlockBaseAddress(buf, 0);
		return buf;
	}
};

PersonVision::PersonVision() : impl(new Impl)
{
	impl->configure(1);
}

void PersonVision::set_quality(int quality)
{
	if (quality != impl->quality)
		impl->configure(quality);
}

int PersonVision::quality() const
{
	return impl->quality;
}

PersonVision::~PersonVision()
{
	if (impl->pool)
		CVPixelBufferPoolRelease(impl->pool);
	impl->request = nil;
	impl->sequence = nil;
}

bool PersonVision::run(const uint8_t *bgra, int w, int h, int stride, std::vector<uint8_t> &mask, int &mw, int &mh)
{
	@autoreleasepool {
		CVPixelBufferRef buf = impl->wrap(bgra, w, h, stride);
		if (!buf)
			return false;
		NSError *err = nil;
		BOOL ok = [impl->sequence performRequests:@[impl->request] onCVPixelBuffer:buf error:&err];
		CVPixelBufferRelease(buf);
		VNPixelBufferObservation *obs = ok ? impl->request.results.firstObject : nil;
		if (!obs)
			return false;
		CVPixelBufferRef m = obs.pixelBuffer;
		CVPixelBufferLockBaseAddress(m, kCVPixelBufferLock_ReadOnly);
		mw = (int)CVPixelBufferGetWidth(m);
		mh = (int)CVPixelBufferGetHeight(m);
		const uint8_t *src = (const uint8_t *)CVPixelBufferGetBaseAddress(m);
		size_t ms = CVPixelBufferGetBytesPerRow(m);
		mask.resize((size_t)mw * mh);
		for (int y = 0; y < mh; y++)
			memcpy(mask.data() + (size_t)y * mw, src + y * ms, mw);
		CVPixelBufferUnlockBaseAddress(m, kCVPixelBufferLock_ReadOnly);
		return mw > 0 && mh > 0;
	}
}
