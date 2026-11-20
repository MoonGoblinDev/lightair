// MediaPipe body and hand tracking shared by every platform (see body-mediapipe.hpp).

#include "body-mediapipe.hpp"

#include <algorithm>
#include <cmath>
#include <cstring>
#include <vector>

namespace beauty {

namespace {

/* BlazePose's 33 landmarks (then 6 auxiliary: 33 hips' centre, 34 the full body's extent, …) */
enum {
	NOSE = 0,
	LEFT_EAR = 7,
	RIGHT_EAR = 8,
	LEFT_SHOULDER = 11,
	RIGHT_SHOULDER = 12,
	LEFT_ELBOW = 13,
	RIGHT_ELBOW = 14,
	LEFT_WRIST = 15,
	RIGHT_WRIST = 16,
	LEFT_PINKY = 17,
	RIGHT_PINKY = 18,
	LEFT_INDEX = 19,
	RIGHT_INDEX = 20,
	LEFT_HIP = 23,
	RIGHT_HIP = 24,
	LEFT_KNEE = 25,
	RIGHT_KNEE = 26,
	LEFT_ANKLE = 27,
	RIGHT_ANKLE = 28,
	AUX_HIPS = 33,
	AUX_EXTENT = 34,
	POSE_POINTS = 39,
};

/* The person detector's anchors (224 input; strides 8, 16 and 32 x3; two a cell each). */
const std::vector<float> &anchors()
{
	static const std::vector<float> a = [] {
		std::vector<float> v;
		const int strides[3] = {8, 16, 32}, per_cell[3] = {2, 2, 6};
		for (int s = 0; s < 3; s++) {
			int fm = (BodyNets::DET + strides[s] - 1) / strides[s];
			for (int y = 0; y < fm; y++)
				for (int x = 0; x < fm; x++)
					for (int k = 0; k < per_cell[s]; k++) {
						v.push_back((x + 0.5f) / fm);
						v.push_back((y + 0.5f) / fm);
					}
		}
		return v;
	}();
	return a;
}

inline float sigmoid(float x)
{
	return 1.0f / (1.0f + std::exp(-std::clamp(x, -60.0f, 60.0f)));
}

/* A crop around a centre, sized to reach a second point, turned so that point is up. */
Roi aligned(float cx, float cy, float tx, float ty, float scale)
{
	Roi r;
	float dx = tx - cx, dy = ty - cy;
	r.cx = cx;
	r.cy = cy;
	r.size = 2.0f * std::hypot(dx, dy) * scale;
	r.rot = std::atan2(dx, -dy);
	return r;
}

void to_frame(const Roi &r, int n, float u, float v, float &x, float &y)
{
	float m[6];
	BodyNets::roi_affine(r, n, m);
	x = m[0] + m[1] * u + m[2] * v;
	y = m[3] + m[4] * u + m[5] * v;
}

} // namespace

void BodyNets::roi_affine(const Roi &r, int n, float m[6])
{
	/* centre + R(rot) * ((u, v) - n/2) * size/n */
	float k = r.size / n, cs = std::cos(r.rot) * k, sn = std::sin(r.rot) * k, h = n / 2.0f;
	m[0] = r.cx - h * (cs - sn);
	m[1] = cs;
	m[2] = -sn;
	m[3] = r.cy - h * (sn + cs);
	m[4] = sn;
	m[5] = cs;
}

/* The person detector on the whole frame (letterboxed, centred): the best person's hips' centre
 * and extent point make the pose crop. */
bool BodyPipeline::detect(const uint8_t *bgra, int w, int h, int stride)
{
	Roi frame;
	frame.cx = w * 0.5f;
	frame.cy = h * 0.5f;
	frame.size = (float)std::max(w, h);
	const float *scores = nullptr, *boxes = nullptr;
	if (!nets->run_detector(bgra, w, h, stride, frame, scores, boxes) || !scores || !boxes)
		return false;
	const auto &a = anchors();
	int n = (int)a.size() / 2, best = -1;
	float best_score = 0.5f;
	for (int i = 0; i < n; i++) {
		float s = sigmoid(scores[i]);
		if (s > best_score) {
			best_score = s;
			best = i;
		}
	}
	if (best < 0)
		return false;
	const float *b = boxes + (size_t)best * 12;
	auto kp = [&](int k, float &x, float &y) {
		float u = b[4 + k * 2] + a[best * 2] * BodyNets::DET, v = b[5 + k * 2] + a[best * 2 + 1] * BodyNets::DET;
		to_frame(frame, BodyNets::DET, u, v, x, y);
	};
	float hx, hy, ex, ey;
	kp(0, hx, hy);
	kp(1, ex, ey);
	pose_roi = aligned(hx, hy, ex, ey, 1.25f);
	return pose_roi.size > 8.0f;
}

bool BodyPipeline::track(const uint8_t *bgra, int w, int h, int stride, bt_body &out)
{
	memset(&out, 0, sizeof(out));
	if (!ready())
		return false;
	/* the detector now and then while following: a crop gone astray (a seated person's hips,
	 * which it's built from, are out of view) is put back */
	if (have_pose && ++since_detect >= 30) {
		Roi kept = pose_roi;
		if (!detect(bgra, w, h, stride))
			pose_roi = kept;
		since_detect = 0;
	}
	if (!have_pose) {
		since_detect = 0;
		if (!detect(bgra, w, h, stride))
			return true; /* (no one in view: a result all the same) */
	}
	const float *lm = nullptr, *world = nullptr;
	float presence = 0;
	if (!nets->run_pose(bgra, w, h, stride, pose_roi, lm, presence, world) || !lm || !world || presence < 0.5f) {
		have_pose = false;
		have_hand[0] = have_hand[1] = false;
		return true;
	}
	/* the landmarks in frame pixels, with their visibility */
	float px[POSE_POINTS][2], vis[POSE_POINTS];
	for (int i = 0; i < POSE_POINTS; i++) {
		to_frame(pose_roi, BodyNets::POSE, lm[i * 5], lm[i * 5 + 1], px[i][0], px[i][1]);
		vis[i] = std::min(sigmoid(lm[i * 5 + 3]), sigmoid(lm[i * 5 + 4]));
	}
	/* world points: metres from the hips' centre, turned back from the crop to the frame's axes
	 * (x right, y down, z away), then ours (x the person's left = the picture's right, y up,
	 * z toward the camera) */
	float cs = std::cos(pose_roi.rot), sn = std::sin(pose_roi.rot);

	/* next frame's crop: from the alignment points, as the detector gives them */
	pose_roi = aligned(px[AUX_HIPS][0], px[AUX_HIPS][1], px[AUX_EXTENT][0], px[AUX_EXTENT][1], 1.25f);
	have_pose = pose_roi.size > 8.0f;
	float wp[POSE_POINTS][3];
	for (int i = 0; i < POSE_POINTS; i++) {
		float x = world[i * 3], y = world[i * 3 + 1], z = world[i * 3 + 2];
		wp[i][0] = cs * x - sn * y;
		wp[i][1] = -(sn * x + cs * y);
		wp[i][2] = -z;
	}

	out.present = true;
	out.has3d = true;
	struct Map {
		int joint, a, b; /* a joint from one landmark, or the middle of two */
	};
	static const Map map[] = {
		{BT_ROOT, LEFT_HIP, RIGHT_HIP},
		{BT_LEFT_HIP, LEFT_HIP, LEFT_HIP},
		{BT_LEFT_KNEE, LEFT_KNEE, LEFT_KNEE},
		{BT_LEFT_ANKLE, LEFT_ANKLE, LEFT_ANKLE},
		{BT_RIGHT_HIP, RIGHT_HIP, RIGHT_HIP},
		{BT_RIGHT_KNEE, RIGHT_KNEE, RIGHT_KNEE},
		{BT_RIGHT_ANKLE, RIGHT_ANKLE, RIGHT_ANKLE},
		{BT_CENTER_SHOULDER, LEFT_SHOULDER, RIGHT_SHOULDER},
		{BT_CENTER_HEAD, LEFT_EAR, RIGHT_EAR},
		{BT_TOP_HEAD, NOSE, NOSE},
		{BT_LEFT_SHOULDER, LEFT_SHOULDER, LEFT_SHOULDER},
		{BT_LEFT_ELBOW, LEFT_ELBOW, LEFT_ELBOW},
		{BT_LEFT_WRIST, LEFT_WRIST, LEFT_WRIST},
		{BT_RIGHT_SHOULDER, RIGHT_SHOULDER, RIGHT_SHOULDER},
		{BT_RIGHT_ELBOW, RIGHT_ELBOW, RIGHT_ELBOW},
		{BT_RIGHT_WRIST, RIGHT_WRIST, RIGHT_WRIST},
	};
	for (const Map &m : map) {
		for (int k = 0; k < 3; k++)
			out.j3[m.joint][k] = (wp[m.a][k] + wp[m.b][k]) * 0.5f;
		out.j3[m.joint][3] = 1.0f;
		out.j2[m.joint][0] = (px[m.a][0] + px[m.b][0]) * 0.5f / w;
		out.j2[m.joint][1] = (px[m.a][1] + px[m.b][1]) * 0.5f / h;
		out.j2[m.joint][2] = std::min(vis[m.a], vis[m.b]);
	}
	/* the spine: halfway from the hips to the shoulders */
	for (int k = 0; k < 3; k++)
		out.j3[BT_SPINE][k] = (out.j3[BT_ROOT][k] + out.j3[BT_CENTER_SHOULDER][k]) * 0.5f;
	out.j3[BT_SPINE][3] = 1.0f;
	out.j2[BT_SPINE][0] = (out.j2[BT_ROOT][0] + out.j2[BT_CENTER_SHOULDER][0]) * 0.5f;
	out.j2[BT_SPINE][1] = (out.j2[BT_ROOT][1] + out.j2[BT_CENTER_SHOULDER][1]) * 0.5f;
	out.j2[BT_SPINE][2] = std::min(out.j2[BT_ROOT][2], out.j2[BT_CENTER_SHOULDER][2]);
	/* (the top of the head isn't one of BlazePose's: the nose stands in, unsure) */
	out.j2[BT_TOP_HEAD][2] = 0;
	out.j3[BT_TOP_HEAD][3] = 0;

	float pose_px[POSE_POINTS * 3];
	for (int i = 0; i < POSE_POINTS; i++) {
		pose_px[i * 3] = px[i][0];
		pose_px[i * 3 + 1] = px[i][1];
		pose_px[i * 3 + 2] = vis[i];
	}
	for (int side = 0; side < 2; side++)
		hand(bgra, w, h, stride, side, pose_px, out);
	return true;
}

/* One hand (0 the person's left): around its own last points if it had them, else around the
 * pose's wrist and knuckles; its points go to out.hand[side]. */
bool BodyPipeline::hand(const uint8_t *bgra, int w, int h, int stride, int side, const float *pose_px, bt_body &out)
{
	const int wrist = side == 0 ? LEFT_WRIST : RIGHT_WRIST, index = side == 0 ? LEFT_INDEX : RIGHT_INDEX,
		  pinky = side == 0 ? LEFT_PINKY : RIGHT_PINKY;
	auto P = [&](int i, int k) { return pose_px[i * 3 + k]; };
	float wx = P(wrist, 0), wy = P(wrist, 1);
	float kx = (P(index, 0) + P(pinky, 0)) * 0.5f, ky = (P(index, 1) + P(pinky, 1)) * 0.5f;
	float palm = std::hypot(kx - wx, ky - wy);
	bool in_view = std::min(P(wrist, 2), std::max(P(index, 2), P(pinky, 2))) > 0.3f && palm > 2.0f;
	/* a crop from its last points that has wandered off the pose's wrist is dropped */
	if (have_hand[side] && in_view && std::hypot(hand_roi[side].cx - wx, hand_roi[side].cy - wy) > hand_roi[side].size)
		have_hand[side] = false;
	if (!have_hand[side]) {
		if (!in_view)
			return false;
		/* the knuckles a palm on from the wrist; the fingers about as far again */
		Roi r = aligned(wx, wy, kx, ky, 1.0f);
		float dx = (kx - wx) / palm, dy = (ky - wy) / palm;
		r.cx = wx + dx * palm * 1.1f;
		r.cy = wy + dy * palm * 1.1f;
		r.size = palm * 3.8f;
		hand_roi[side] = r;
	}
	const float *lm = nullptr, *world = nullptr;
	float presence = 0;
	if (!nets->run_hand(bgra, w, h, stride, hand_roi[side], lm, presence, world) || !lm || presence < 0.5f) {
		have_hand[side] = false;
		return false;
	}
	float hx[BT_HAND_POINTS], hy[BT_HAND_POINTS];
	float cs0 = std::cos(hand_roi[side].rot), sn0 = std::sin(hand_roi[side].rot);
	for (int i = 0; i < BT_HAND_POINTS; i++) {
		to_frame(hand_roi[side], BodyNets::HAND, lm[i * 3], lm[i * 3 + 1], hx[i], hy[i]);
		out.hand[side][i][0] = hx[i] / w;
		out.hand[side][i][1] = hy[i] / h;
		out.hand[side][i][2] = presence;
		if (world) {
			/* turned back from the crop, then our axes (x right, y up, z toward the camera) */
			float x = world[i * 3], y = world[i * 3 + 1], z = world[i * 3 + 2];
			out.hand3d[side][i][0] = cs0 * x - sn0 * y;
			out.hand3d[side][i][1] = -(sn0 * x + cs0 * y);
			out.hand3d[side][i][2] = -z;
		}
	}
	out.hand_present[side] = true;
	/* next frame's crop: around these points, upright from the wrist to the middle knuckle */
	Roi r = aligned(hx[0], hy[0], hx[9], hy[9], 1.0f);
	float cs = std::cos(r.rot), sn = std::sin(r.rot);
	float lo_u = 1e9f, hi_u = -1e9f, lo_v = 1e9f, hi_v = -1e9f;
	for (int i = 0; i < BT_HAND_POINTS; i++) {
		/* in the crop's turned axes */
		float dx = hx[i] - hx[0], dy = hy[i] - hy[0];
		float u = cs * dx + sn * dy, v = -sn * dx + cs * dy;
		lo_u = std::min(lo_u, u);
		hi_u = std::max(hi_u, u);
		lo_v = std::min(lo_v, v);
		hi_v = std::max(hi_v, v);
	}
	float cu = (lo_u + hi_u) * 0.5f, cv = (lo_v + hi_v) * 0.5f;
	r.cx = hx[0] + cs * cu - sn * cv;
	r.cy = hy[0] + sn * cu + cs * cv;
	r.size = std::max(hi_u - lo_u, hi_v - lo_v) * 1.9f;
	hand_roi[side] = r;
	have_hand[side] = r.size > 8.0f;
	return true;
}

} // namespace beauty
