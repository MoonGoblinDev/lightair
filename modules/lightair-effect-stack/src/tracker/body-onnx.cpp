// The body tracker's networks on ONNX Runtime (Windows: DirectML, CPU as a fallback):
// data/models/*.onnx from OpenCV Zoo.
//   person_detection_mediapipe_2023mar.onnx    [1,3,224,224] RGB -1..1 -> scores, boxes
//   pose_estimation_mediapipe_2023mar.onnx     [1,256,256,3] RGB 0..1  -> landmarks, presence, mask,
//                                              heatmap, world
//   handpose_estimation_mediapipe_2023feb.onnx [1,224,224,3] RGB 0..1  -> landmarks, presence,
//                                              handedness, world

#include <string>
#include <vector>

#include "body-mediapipe.hpp"
#include "onnx-common.hpp"

namespace beauty {
namespace {

class OnnxBodyNets : public BodyNets {
public:
	explicit OnnxBodyNets(const std::string &dir)
	{
		try {
			det = ort_model(dir + "/person_detection_mediapipe_2023mar.onnx", true);
			pose = ort_model(dir + "/pose_estimation_mediapipe_2023mar.onnx", true);
			hand_net = ort_model(dir + "/handpose_estimation_mediapipe_2023feb.onnx", true);
			ok = det && pose && hand_net;
			ort_log(ok ? "body: MediaPipe BlazePose and hands (%s)" : "body models failed to load",
				pose && pose->accelerated ? ort_accel_name() : "CPU");
		} catch (const Ort::Exception &e) {
			ort_log("ONNX Runtime error: %s", e.what());
		}
	}
	bool ready() const override { return ok; }

	bool run_detector(const uint8_t *bgra, int w, int h, int stride, const Roi &r, const float *&scores,
			  const float *&boxes) override
	{
		const int n = DET;
		det_in.assign((size_t)3 * n * n, 0.0f);
		fill_crop(bgra, w, h, stride, r, n, [&](int u, int v, const float *px) {
			for (int c = 0; c < 3; c++) /* RGB planes, -1..1 */
				det_in[(size_t)c * n * n + (size_t)v * n + u] = px[2 - c] / 127.5f - 1.0f;
		});
		int64_t shape[4] = {1, 3, n, n};
		if (!run(*det, det_in, shape, det_out))
			return false;
		scores = output(*det, det_out, "Identity_1:0");
		boxes = output(*det, det_out, "Identity:0");
		return scores && boxes;
	}

	bool run_pose(const uint8_t *bgra, int w, int h, int stride, const Roi &r, const float *&lm, float &presence,
		      const float *&world) override
	{
		if (!run_nhwc(*pose, bgra, w, h, stride, r, POSE, pose_in, pose_out))
			return false;
		lm = output(*pose, pose_out, "Identity");
		const float *p = output(*pose, pose_out, "Identity_1");
		world = output(*pose, pose_out, "Identity_4");
		presence = p ? p[0] : 0;
		return lm && p && world;
	}

	bool run_hand(const uint8_t *bgra, int w, int h, int stride, const Roi &r, const float *&lm, float &presence,
		      const float *&world) override
	{
		if (!run_nhwc(*hand_net, bgra, w, h, stride, r, HAND, hand_in, hand_out))
			return false;
		lm = output(*hand_net, hand_out, "Identity");
		const float *p = output(*hand_net, hand_out, "Identity_1");
		world = output(*hand_net, hand_out, "Identity_3");
		presence = p ? p[0] : 0;
		return lm && p;
	}

private:
	static bool run(const OrtModel &m, std::vector<float> &in, const int64_t shape[4], std::vector<Ort::Value> &out)
	{
		try {
			Ort::MemoryInfo mem = Ort::MemoryInfo::CreateCpu(OrtArenaAllocator, OrtMemTypeDefault);
			Ort::Value t = Ort::Value::CreateTensor<float>(mem, in.data(), in.size(), shape, 4);
			out = m.run(t);
			return true;
		} catch (const Ort::Exception &e) {
			ort_log("body model failed: %s", e.what());
			return false;
		}
	}

	/* [1,n,n,3] RGB 0..1 */
	static bool run_nhwc(const OrtModel &m, const uint8_t *bgra, int w, int h, int stride, const Roi &r, int n,
			     std::vector<float> &in, std::vector<Ort::Value> &out)
	{
		in.resize((size_t)n * n * 3);
		fill_crop(bgra, w, h, stride, r, n, [&](int u, int v, const float *px) {
			float *o = &in[((size_t)v * n + u) * 3];
			o[0] = px[2] / 255.f;
			o[1] = px[1] / 255.f;
			o[2] = px[0] / 255.f;
		});
		int64_t shape[4] = {1, n, n, 3};
		return run(m, in, shape, out);
	}

	static const float *output(const OrtModel &m, std::vector<Ort::Value> &out, const char *name)
	{
		for (size_t i = 0; i < m.out_names.size() && i < out.size(); i++)
			if (m.out_names[i] == name)
				return out[i].GetTensorData<float>();
		return nullptr;
	}

	bool ok = false;
	OrtModelPtr det, pose, hand_net;
	std::vector<float> det_in, pose_in, hand_in;
	std::vector<Ort::Value> det_out, pose_out, hand_out;
};

} // namespace

std::unique_ptr<BodyNets> create_body_nets(const std::string &models_dir)
{
	return std::make_unique<OnnxBodyNets>(models_dir);
}

} // namespace beauty
