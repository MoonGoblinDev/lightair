// The body tracker's C interface (body-tracker.h): MediaPipe body and hands (body-mediapipe.cpp)
// on a thread of its own, one frame at a time; frames that come while it's busy are dropped.

#include <condition_variable>
#include <cstring>
#include <mutex>
#include <thread>
#include <vector>

#include "body-mediapipe.hpp"
#include "body-tracker.h"

using namespace beauty;

struct body_tracker {
	std::mutex m;
	std::condition_variable cv;
	std::thread worker;
	bool quit = false;
	/* a frame waiting to be analysed */
	bool pending = false;
	std::vector<uint8_t> frame;
	uint32_t w = 0, h = 0;
	uint64_t frame_id = 0;
	/* the latest result */
	bool have = false;
	bt_body latest{};

	void run(std::string models)
	{
		BodyPipeline pipeline(create_body_nets(models));
		std::vector<uint8_t> work;
		for (;;) {
			uint32_t fw, fh;
			uint64_t id;
			{
				std::unique_lock<std::mutex> g(m);
				cv.wait(g, [&] { return quit || pending; });
				if (quit)
					return;
				work.swap(frame);
				fw = w;
				fh = h;
				id = frame_id;
			}
			bt_body out{};
			pipeline.track(work.data(), (int)fw, (int)fh, (int)fw * 4, out);
			out.frame_id = id;
			std::lock_guard<std::mutex> g(m);
			latest = out;
			have = true;
			pending = false;
		}
	}
};

extern "C" body_tracker_t *body_tracker_create(const char *models_dir)
{
	auto *bt = new body_tracker;
	std::string dir = models_dir ? models_dir : "";
	/* (the models load on the worker, so this returns at once) */
	bt->worker = std::thread([bt, dir] { bt->run(dir); });
	return bt;
}

extern "C" void body_tracker_destroy(body_tracker_t *bt)
{
	if (!bt)
		return;
	{
		std::lock_guard<std::mutex> g(bt->m);
		bt->quit = true;
	}
	bt->cv.notify_all();
	if (bt->worker.joinable())
		bt->worker.join();
	delete bt;
}

extern "C" bool body_tracker_submit_bgra(body_tracker_t *bt, const uint8_t *data, uint32_t linesize, uint32_t width,
					 uint32_t height, uint64_t frame_id)
{
	if (!bt || !data || !width || !height || linesize < width * 4)
		return false;
	{
		std::lock_guard<std::mutex> g(bt->m);
		if (bt->pending)
			return false;
		bt->frame.resize((size_t)width * height * 4);
		for (uint32_t y = 0; y < height; y++)
			memcpy(bt->frame.data() + (size_t)y * width * 4, data + (size_t)y * linesize, (size_t)width * 4);
		bt->w = width;
		bt->h = height;
		bt->frame_id = frame_id;
		bt->pending = true;
	}
	bt->cv.notify_one();
	return true;
}

extern "C" bool body_tracker_get(body_tracker_t *bt, bt_body *out)
{
	if (!bt || !out)
		return false;
	std::lock_guard<std::mutex> g(bt->m);
	if (bt->have)
		*out = bt->latest;
	return bt->have;
}
