// Person segmentation with Apple Vision (macOS): a finer mask than the 256px MediaPipe one, for
// the background effects. Runs on the segmentation worker; one instance per tracker.
#pragma once

#include <cstdint>
#include <memory>
#include <vector>

class PersonVision {
public:
	PersonVision();
	~PersonVision();
	/* 0 fast, 1 balanced (Apple's choice for video), 2 accurate */
	void set_quality(int quality);
	int quality() const;
	/* BGRA frame in, 8-bit person mask out (mw x mh, top-left origin). */
	bool run(const uint8_t *bgra, int w, int h, int stride, std::vector<uint8_t> &mask, int &mw, int &mh);

private:
	struct Impl;
	std::unique_ptr<Impl> impl;
};
