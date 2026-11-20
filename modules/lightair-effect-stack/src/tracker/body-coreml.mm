// The body tracker's networks on Core ML (macOS): data/models/coreml/*.mlpackage, converted by
// tools/convert-coreml.py and compiled into the plugin. Pose and hands run in fp16 on the Neural
// Engine; the person detector in fp32 (its raw scores pass fp16's range), only to find someone.

#include <string>
#include <vector>

#include "body-mediapipe.hpp"
#include "coreml-common.hpp"
#include "tracker-util.hpp"

namespace beauty {
namespace {

class CoreMLBodyNets : public BodyNets {
public:
	explicit CoreMLBodyNets(const std::string &dir)
	{
		bool d = det.open(dir + "/person_detection_mediapipe_2023mar.mlmodelc", DET, DET);
		bool p = pose.open(dir + "/pose_estimation_mediapipe_2023mar.mlmodelc", POSE, POSE);
		bool h = hand_net.open(dir + "/handpose_estimation_mediapipe_2023feb.mlmodelc", HAND, HAND);
		ok = d && p && h;
		tracker_log(ok ? "body: MediaPipe BlazePose and hands (Core ML)" : "body models failed to load");
	}
	bool ready() const override { return ok; }

	bool run_detector(const uint8_t *bgra, int w, int h, int stride, const Roi &r, const float *&scores,
			  const float *&boxes) override
	{
		if (!fill(det, bgra, w, h, stride, r, DET) || !det.run() || !det.read("scores", det_scores) ||
		    !det.read("boxes", det_boxes))
			return false;
		scores = det_scores.data();
		boxes = det_boxes.data();
		return det_scores.size() >= 2254 && det_boxes.size() >= 2254 * 12;
	}

	bool run_pose(const uint8_t *bgra, int w, int h, int stride, const Roi &r, const float *&lm, float &presence,
		      const float *&world) override
	{
		if (!fill(pose, bgra, w, h, stride, r, POSE) || !pose.run() || !pose.read("landmarks", pose_lm) ||
		    !pose.read("presence", pose_pres) || !pose.read("world", pose_world) || pose_lm.size() < 195 ||
		    pose_pres.empty() || pose_world.size() < 117)
			return false;
		lm = pose_lm.data();
		presence = pose_pres[0];
		world = pose_world.data();
		return true;
	}

	bool run_hand(const uint8_t *bgra, int w, int h, int stride, const Roi &r, const float *&lm, float &presence,
		      const float *&world) override
	{
		if (!fill(hand_net, bgra, w, h, stride, r, HAND) || !hand_net.run() || !hand_net.read("landmarks", hand_lm) ||
		    !hand_net.read("presence", hand_pres) || !hand_net.read("world", hand_world) || hand_lm.size() < 63 ||
		    hand_pres.empty())
			return false;
		lm = hand_lm.data();
		presence = hand_pres[0];
		world = hand_world.data();
		return true;
	}

private:
	static bool fill(CoreMLRunner &net, const uint8_t *bgra, int w, int h, int stride, const Roi &r, int n)
	{
		size_t ds;
		uint8_t *dst = net.lock_input(ds);
		if (!dst)
			return false;
		fill_crop(bgra, w, h, stride, r, n, [&](int u, int v, const float *px) {
			uint8_t *o = dst + (size_t)v * ds + (size_t)u * 4;
			o[0] = (uint8_t)(px[0] + 0.5f);
			o[1] = (uint8_t)(px[1] + 0.5f);
			o[2] = (uint8_t)(px[2] + 0.5f);
			o[3] = 255;
		});
		net.unlock_input();
		return true;
	}

	bool ok = false;
	CoreMLRunner det, pose, hand_net;
	std::vector<float> det_scores, det_boxes, pose_lm, pose_pres, pose_world, hand_lm, hand_pres, hand_world;
};

} // namespace

std::unique_ptr<BodyNets> create_body_nets(const std::string &models_dir)
{
	return std::make_unique<CoreMLBodyNets>(models_dir);
}

} // namespace beauty
