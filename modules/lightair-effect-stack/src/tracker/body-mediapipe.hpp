// Body and hand tracking for avatars with MediaPipe's BlazePose and hand landmark models (from
// OpenCV Zoo), the same on every platform: the person detector finds someone when no one is
// followed, the pose model then runs every frame on a rotated crop from its own previous
// alignment points, and each hand's model on a crop around the pose's wrist and knuckles (then
// around its own last points). Subclasses only run the three networks (Core ML on macOS, ONNX
// Runtime elsewhere).
#pragma once

#include <cstdint>
#include <memory>
#include <string>

#include "body-tracker.h"

namespace beauty {

/* A square crop: centre, side (frame pixels) and turn (radians; 0 upright). */
struct Roi {
	float cx = 0, cy = 0, size = 0, rot = 0;
};

class BodyNets {
public:
	virtual ~BodyNets() = default;
	virtual bool ready() const = 0;

	static constexpr int DET = 224, POSE = 256, HAND = 224;

	/* The networks on a crop (sampled with fill_crop). Pointers stay valid until the next call
	 * of the same network.
	 * detector: scores [2254] (logits), boxes [2254 x 12] (crop pixels from each anchor)
	 * pose:     landmarks [39 x 5] (crop pixels, z, visibility and presence logits), presence
	 *           (0..1), world [39 x 3] (metres, the crop's axes)
	 * hand:     landmarks [21 x 3] (crop pixels), presence (0..1), world [21 x 3] */
	virtual bool run_detector(const uint8_t *bgra, int w, int h, int stride, const Roi &r, const float *&scores,
				  const float *&boxes) = 0;
	virtual bool run_pose(const uint8_t *bgra, int w, int h, int stride, const Roi &r, const float *&lm,
			      float &presence, const float *&world) = 0;
	virtual bool run_hand(const uint8_t *bgra, int w, int h, int stride, const Roi &r, const float *&lm,
			      float &presence, const float *&world) = 0;

	/* crop position (u, v) of an n x n crop -> frame position */
	static void roi_affine(const Roi &r, int n, float m[6]);
	/* The crop's pixels (B, G, R; black outside the frame) for each (u, v), in rows. */
	template<class F> static void fill_crop(const uint8_t *bgra, int w, int h, int stride, const Roi &r, int n, F put);
};

std::unique_ptr<BodyNets> create_body_nets(const std::string &models_dir);

/* Follows one person from frame to frame. */
class BodyPipeline {
public:
	explicit BodyPipeline(std::unique_ptr<BodyNets> nets) : nets(std::move(nets)) {}
	bool ready() const { return nets && nets->ready(); }
	/* The body (and hands) in a BGRA frame. */
	bool track(const uint8_t *bgra, int w, int h, int stride, bt_body &out);

private:
	bool detect(const uint8_t *bgra, int w, int h, int stride);
	bool hand(const uint8_t *bgra, int w, int h, int stride, int side, const float *pose_px, bt_body &out);

	std::unique_ptr<BodyNets> nets;
	bool have_pose = false;
	Roi pose_roi;
	/* frames followed from the pose's own points since the detector last looked */
	int since_detect = 0;
	bool have_hand[2] = {};
	Roi hand_roi[2];
};

} // namespace beauty

#include "tracker-util.hpp"

template<class F>
void beauty::BodyNets::fill_crop(const uint8_t *bgra, int w, int h, int stride, const Roi &r, int n, F put)
{
	float m[6], px[3];
	roi_affine(r, n, m);
	for (int v = 0; v < n; v++)
		for (int u = 0; u < n; u++) {
			float x = m[0] + m[1] * (u + 0.5f) + m[2] * (v + 0.5f);
			float y = m[3] + m[4] * (u + 0.5f) + m[5] * (v + 0.5f);
			if (x < 0 || y < 0 || x >= w || y >= h) {
				px[0] = px[1] = px[2] = 0;
			} else {
				sample_bgra(bgra, w, h, stride, x, y, px);
			}
			put(u, v, px);
		}
}
