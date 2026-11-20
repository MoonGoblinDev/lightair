#pragma once

/* Body and hand tracking for avatars (MediaPipe BlazePose and hand landmarks, every platform:
 * src/tracker/body-mediapipe.cpp). A frame in, the latest body out, analysed on a thread of its
 * own (frames that arrive while it's busy are dropped). */

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Joints, the person's own left and right. */
enum bt_joint_name {
	BT_ROOT,
	BT_LEFT_HIP,
	BT_LEFT_KNEE,
	BT_LEFT_ANKLE,
	BT_RIGHT_HIP,
	BT_RIGHT_KNEE,
	BT_RIGHT_ANKLE,
	BT_SPINE,
	BT_CENTER_SHOULDER,
	BT_CENTER_HEAD,
	BT_TOP_HEAD,
	BT_LEFT_SHOULDER,
	BT_LEFT_ELBOW,
	BT_LEFT_WRIST,
	BT_RIGHT_SHOULDER,
	BT_RIGHT_ELBOW,
	BT_RIGHT_WRIST,
	BT_JOINT_COUNT
};

#define BT_HAND_POINTS 21

typedef struct {
	bool present;
	/* 3D joints are known (else only the picture's 2D ones). */
	bool has3d;
	uint64_t frame_id;
	/* metres, from the hips' centre: x toward the person's left (the picture's right), y up,
	 * z toward the camera; w: 1 when known */
	float j3[BT_JOINT_COUNT][4];
	/* in the picture: x, y (0..1, y down), confidence */
	float j2[BT_JOINT_COUNT][3];
	/* hands: 0 the person's left, 1 right; points in MediaPipe's order (wrist, thumb ×4,
	 * index ×4, middle ×4, ring ×4, little ×4): x, y (0..1, y down), confidence */
	bool hand_present[2];
	float hand[2][BT_HAND_POINTS][3];
	/* the hands in 3D: metres from the hand's centre, our axes (as j3) */
	float hand3d[2][BT_HAND_POINTS][3];
} bt_body;

typedef struct body_tracker body_tracker_t;

/* models_dir: the folder with the models (as the face tracker's). */
body_tracker_t *body_tracker_create(const char *models_dir);
void body_tracker_destroy(body_tracker_t *bt);
/* Copies the BGRA frame and analyses it in the background. False when busy (dropped). */
bool body_tracker_submit_bgra(body_tracker_t *bt, const uint8_t *data, uint32_t linesize, uint32_t width,
			      uint32_t height, uint64_t frame_id);
/* The latest result; false before the first. */
bool body_tracker_get(body_tracker_t *bt, bt_body *out);

#ifdef __cplusplus
}
#endif
