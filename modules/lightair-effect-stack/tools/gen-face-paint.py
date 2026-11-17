#!/usr/bin/env python3
"""Bake Dewy's face-paint map and the face-mesh source file from MediaPipe's canonical face model.

    python3 tools/gen-face-paint.py art/face-mesh/canonical_face_model.obj [--preview out.png]

Writes
    data/makeup/face-paint.png   RGBA in the mesh's UV space: R contour, G highlight, B eyeliner, A brow
    src/face-mesh-data.h         UVs, triangles and their front-view winding, for rendering the mesh

Every texel of the map knows where it sits on the canonical face (in centimetres: x towards the
subject's left, y up, z towards the camera; eye corners are 6.3 cm apart), so each product is
drawn the way a makeup artist would place it - measured on the face, along the face's own
structure - and then rides on the tracked mesh: it turns with the head and stays on the skin.
"""
import argparse
import os
import sys

import numpy as np
from PIL import Image

N = 2048  # map size
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def load_obj(path):
    V, T, F = [], [], []
    for line in open(path):
        p = line.split()
        if not p:
            continue
        if p[0] == "v":
            V.append([float(x) for x in p[1:4]])
        elif p[0] == "vt":
            T.append([float(x) for x in p[1:3]])
        elif p[0] == "f":
            F.append([[int(x) for x in q.split("/")[:2]] for q in p[1:]])
    V = np.array(V)
    uv = np.zeros((len(V), 2))
    for f in F:
        for vi, ti in f:
            uv[vi - 1] = T[ti - 1]
    tris = np.array([[vi - 1 for vi, _ in f] for f in F])
    return V, uv, tris


def rasterize(V, uv, tris):
    """Per texel: canonical 3D position (NaN outside the mesh). Row 0 = top (v = 1)."""
    P = np.full((N, N, 3), np.nan)
    px = np.stack([uv[:, 0] * N, (1.0 - uv[:, 1]) * N], 1)
    for t in tris:
        a, b, c = px[t]
        x0, x1 = int(max(np.floor(min(a[0], b[0], c[0])), 0)), int(min(np.ceil(max(a[0], b[0], c[0])), N - 1))
        y0, y1 = int(max(np.floor(min(a[1], b[1], c[1])), 0)), int(min(np.ceil(max(a[1], b[1], c[1])), N - 1))
        if x1 < x0 or y1 < y0:
            continue
        xs, ys = np.meshgrid(np.arange(x0, x1 + 1) + 0.5, np.arange(y0, y1 + 1) + 0.5)
        d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1])
        if abs(d) < 1e-9:
            continue
        l0 = ((b[1] - c[1]) * (xs - c[0]) + (c[0] - b[0]) * (ys - c[1])) / d
        l1 = ((c[1] - a[1]) * (xs - c[0]) + (a[0] - c[0]) * (ys - c[1])) / d
        l2 = 1 - l0 - l1
        inside = (l0 >= -1e-4) & (l1 >= -1e-4) & (l2 >= -1e-4)
        pos = l0[..., None] * V[t[0]] + l1[..., None] * V[t[1]] + l2[..., None] * V[t[2]]
        sub = P[y0:y1 + 1, x0:x1 + 1]
        sub[inside] = pos[inside]
    return P


def surface(V, tris, x, y):
    """Front-most point of the canonical face at front-view position (x, y)."""
    best = None
    for t in tris:
        a, b, c = V[t]
        d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1])
        if abs(d) < 1e-9:
            continue
        l0 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / d
        l1 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / d
        l2 = 1 - l0 - l1
        if min(l0, l1, l2) >= -1e-6:
            p = l0 * a + l1 * b + l2 * c
            if best is None or p[2] > best[2]:
                best = p
    if best is None:
        raise ValueError(f"({x}, {y}) is off the face")
    return best


def smoothstep(e0, e1, x):
    t = np.clip((x - e0) / (e1 - e0), 0.0, 1.0)
    return t * t * (3 - 2 * t)


def polyline(P, pts, sigma0, sigma1, s0, s1):
    """Soft stroke along a polyline on the surface (3D distances), with soft round ends. Width
    (sigma) and strength run from the first point to the last."""
    pts = np.asarray(pts, float)
    seg = pts[1:] - pts[:-1]
    seglen = np.linalg.norm(seg, axis=1)
    cum = np.concatenate([[0], np.cumsum(seglen)])
    total = cum[-1]
    best_d = np.full(P.shape[:2], np.inf)
    best_t = np.zeros(P.shape[:2])
    for i in range(len(seg)):
        q = P - pts[i]
        u = np.clip(np.einsum("ijk,k->ij", q, seg[i]) / max(seglen[i] ** 2, 1e-9), 0, 1)
        d = np.linalg.norm(q - u[..., None] * seg[i], axis=2)
        m = d < best_d
        best_d[m] = d[m]
        best_t[m] = (cum[i] + u[m] * seglen[i]) / total
    sigma = sigma0 + (sigma1 - sigma0) * best_t
    s = s0 + (s1 - s0) * best_t
    return np.nan_to_num(s * np.exp(-0.5 * (best_d / sigma) ** 2))


def blob(P, c, sigma, s, squash=(1.0, 1.0, 1.0)):
    q = (P - np.asarray(c)) / np.asarray(squash)
    return np.nan_to_num(s * np.exp(-0.5 * (np.linalg.norm(q, axis=2) / sigma) ** 2))


def poly_fill(P, poly_xy, feather):
    """Inside a front-view polygon, with a soft edge `feather` cm wide."""
    x, y = P[..., 0], P[..., 1]
    poly = np.asarray(poly_xy, float)
    inside = np.zeros(P.shape[:2], bool)
    dmin = np.full(P.shape[:2], np.inf)
    n = len(poly)
    for i in range(n):
        (x0, y0), (x1, y1) = poly[i], poly[(i + 1) % n]
        cond = ((y0 > y) != (y1 > y)) & (x < (x1 - x0) * (y - y0) / ((y1 - y0) + 1e-12) + x0)
        inside ^= np.nan_to_num(cond).astype(bool)
        ex, ey = x1 - x0, y1 - y0
        u = np.clip(((x - x0) * ex + (y - y0) * ey) / (ex * ex + ey * ey), 0, 1)
        dmin = np.minimum(dmin, np.hypot(x - x0 - u * ex, y - y0 - u * ey))
    sd = np.where(inside, dmin, -dmin)
    return np.nan_to_num(smoothstep(-feather, feather, sd))


def build(V, uv, tris):
    P = rasterize(V, uv, tris)
    S = lambda x, y: surface(V, tris, x, y)
    contour = np.zeros((N, N))
    highlight = np.zeros((N, N))
    liner = np.zeros((N, N))
    brow = np.zeros((N, N))

    for s in (-1.0, 1.0):  # subject's right, left
        R = s < 0
        # ---- contour: a shadow under the cheekbone from the ear towards the mouth corner,
        # stopping under the outer corner of the eye; the temples; the jaw edge; the nose sides
        contour = np.maximum(contour, polyline(P, [S(s * 7.1, 0.2), S(s * 6.0, -0.95), S(s * 4.7, -2.15)],
                                               0.66, 0.5, 1.0, 0.8))
        contour = np.maximum(contour, polyline(P, [S(s * 6.1, 5.0), S(s * 6.8, 3.6), S(s * 7.2, 2.0)],
                                               0.65, 0.55, 0.75, 0.6))
        jaw_ids = [132, 58, 172, 136, 150, 149] if R else [361, 288, 397, 365, 379, 378]
        jaw = [V[i] + np.array([-s * 0.25, 0.35, 0.25]) for i in jaw_ids]  # just inside the edge
        contour = np.maximum(contour, polyline(P, jaw, 0.55, 0.45, 0.8, 0.25))
        contour = np.maximum(contour, polyline(P, [S(s * 0.72, 2.6), S(s * 0.8, 1.3), S(s * 0.95, 0.15)],
                                               0.18, 0.2, 0.4, 0.35))

        # ---- highlight: cheekbone tops, under the brow tail, the inner eye corner
        highlight = np.maximum(highlight, polyline(P, [S(s * 3.2, 1.25), S(s * 4.6, 1.5), S(s * 6.0, 2.05)],
                                                   0.42, 0.38, 1.0, 0.7))
        lower_brow = [46, 53, 52] if R else [276, 283, 282]
        highlight = np.maximum(highlight, polyline(P, [V[i] + np.array([0, -0.42, 0]) for i in lower_brow],
                                                   0.2, 0.18, 0.55, 0.4))
        highlight = np.maximum(highlight, blob(P, S(s * 1.55, 2.55), 0.16, 0.45))

        # ---- eyeliner: on the upper lid, hugging the lash line and widening outwards, with a
        # short wing lifted towards the end of the brow
        lid_ids = [133, 173, 157, 158, 159, 160, 161, 246, 33] if R else [362, 398, 384, 385, 386, 387, 388, 466, 263]
        lid = V[lid_ids][:, :2]  # inner -> outer, front view
        order = np.argsort(lid[:, 0] * s)
        lx, ly = lid[order, 0] * s, lid[order, 1]
        x, y = P[..., 0] * s, P[..., 1]
        within = (x >= lx[0]) & (x <= lx[-1])
        lid_y = np.interp(x, lx, ly)
        t = np.clip((x - lx[0]) / (lx[-1] - lx[0]), 0, 1)
        thick = 0.03 + 0.085 * t ** 1.4
        h = y - lid_y
        band = smoothstep(-0.03, 0.0, h) * (1 - smoothstep(thick - 0.025, thick + 0.025, h))
        band *= smoothstep(0.0, 0.1, t) * np.nan_to_num(within).astype(float)
        corner = V[33 if R else 263][:2] * np.array([s, 1])
        tip = corner + np.array([0.42, 0.2])
        x_top = corner[0] - 0.32
        t_top = np.clip((x_top - lx[0]) / (lx[-1] - lx[0]), 0, 1)
        wing_poly = np.array([[x_top, np.interp(x_top, lx, ly) + 0.03 + 0.085 * t_top ** 1.4],
                              tip, corner + np.array([0.02, -0.02])])
        wing_poly[:, 0] *= s
        wing = poly_fill(P, wing_poly, 0.025)
        liner = np.maximum(liner, np.nan_to_num(np.maximum(band, wing)))

        # ---- brow: the brow's own shape (upper and lower outline), softer and thinner at the tail
        up = [70, 63, 105, 66, 107] if R else [300, 293, 334, 296, 336]
        lo = [46, 53, 52, 65, 55] if R else [276, 283, 282, 295, 285]
        poly = [V[i][:2] for i in up] + [V[i][:2] for i in reversed(lo)]
        fill = poly_fill(P, poly, 0.1)
        xi = P[..., 0] * s
        tail = smoothstep(V[up[0]][0] * s - 0.2, V[up[0]][0] * s + 0.9, xi)  # 0 at the tail end
        head = 1 - 0.35 * smoothstep(V[up[-1]][0] * s + 0.45, V[up[-1]][0] * s - 0.1, xi)  # softer front
        brow = np.maximum(brow, np.nan_to_num(fill * (0.55 + 0.45 * tail) * head))

    # ---- centre-line highlights: nose bridge, forehead, cupid's bow, chin
    highlight = np.maximum(highlight, polyline(P, [V[168], V[6], V[197], V[195]], 0.18, 0.2, 0.75, 0.8))
    highlight = np.maximum(highlight, blob(P, S(0.0, 5.6), 0.95, 0.5, (1.0, 1.2, 1.0)))
    highlight = np.maximum(highlight, blob(P, V[0] + np.array([0, 0.28, 0.05]), 0.2, 0.6, (1.4, 1.0, 1.0)))
    highlight = np.maximum(highlight, blob(P, S(0.0, -7.6), 0.5, 0.55, (1.3, 1.0, 1.0)))

    contour *= np.clip(1.0 - highlight * 1.5, 0, 1)  # never both on one spot
    rgba = np.stack([contour, highlight, liner, brow], 2)
    rgba = np.clip(np.nan_to_num(rgba), 0, 1)
    # bleed one ring of texels past the mesh's UV edge so bilinear sampling never pulls in black
    valid = ~np.isnan(P[..., 0])
    for _ in range(4):
        grown = rgba.copy()
        for dy, dx in ((0, 1), (0, -1), (1, 0), (-1, 0)):
            sh = np.roll(np.roll(rgba, dy, 0), dx, 1)
            shv = np.roll(np.roll(valid, dy, 0), dx, 1)
            take = ~valid & shv
            grown[take] = np.maximum(grown[take], sh[take])
        valid = valid | np.roll(valid, 1, 0) | np.roll(valid, -1, 0) | np.roll(valid, 1, 1) | np.roll(valid, -1, 1)
        rgba = grown
    return P, rgba


def write_header(V, uv, tris, path):
    # front-view winding (image space: x right, y down) of every triangle; a tracked triangle
    # whose winding has flipped is facing away from the camera
    img = np.stack([V[:, 0], -V[:, 1]], 1)
    a, b, c = img[tris[:, 0]], img[tris[:, 1]], img[tris[:, 2]]
    area = (b[:, 0] - a[:, 0]) * (c[:, 1] - a[:, 1]) - (b[:, 1] - a[:, 1]) * (c[:, 0] - a[:, 0])
    with open(path, "w") as f:
        f.write("/* Generated by tools/gen-face-paint.py from MediaPipe's canonical face model\n"
                " * (Apache-2.0, see data/models/NOTICE.md). Do not edit. */\n#pragma once\n\n")
        f.write(f"#define FACE_MESH_VERTS {len(V)}\n#define FACE_MESH_TRIS {len(tris)}\n\n")
        f.write("/* texture coordinates (u, v), v = 0 at the top of face-paint.png */\n")
        f.write("static const float face_mesh_uv[FACE_MESH_VERTS][2] = {\n")
        for u, v in uv:
            f.write(f"\t{{{u:.6f}f, {1.0 - v:.6f}f}},\n")
        f.write("};\n\nstatic const unsigned short face_mesh_tris[FACE_MESH_TRIS][3] = {\n")
        for t in tris:
            f.write(f"\t{{{t[0]}, {t[1]}, {t[2]}}},\n")
        f.write("};\n\n/* +1 / -1: winding in a straight-on view (image space, y down) */\n")
        f.write("static const signed char face_mesh_front[FACE_MESH_TRIS] = {\n")
        for i in range(0, len(area), 24):
            f.write("\t" + ", ".join("1" if x > 0 else "-1" for x in area[i:i + 24]) + ",\n")
        f.write("};\n")


def preview(P, rgba, path):
    """Front view of the canonical face with the paint on it (for checking placement)."""
    W, H = 900, 1100
    img = np.full((H, W, 3), 245.0)
    zbuf = np.full((H, W), -np.inf)
    valid = ~np.isnan(P[..., 0])
    ys, xs = np.nonzero(valid)
    p = P[ys, xs]
    sx = ((p[:, 0] + 8.5) / 17.0 * W).astype(int)
    sy = ((9.8 - p[:, 1]) / 20.0 * H).astype(int)
    ok = (sx >= 0) & (sx < W) & (sy >= 0) & (sy < H)
    order = np.argsort(p[:, 2])
    for i in order[ok[order]]:
        if p[i, 2] > zbuf[sy[i], sx[i]]:
            zbuf[sy[i], sx[i]] = p[i, 2]
            c, hl, ln, br = rgba[ys[i], xs[i]]
            skin = np.array([224.0, 186, 160]) * (0.75 + 0.03 * p[i, 2])
            col = skin * (1 - 0.55 * c) + c * 0.55 * np.array([120.0, 80, 60])
            col = col * (1 - 0.6 * hl) + hl * 0.6 * np.array([255.0, 250, 240])
            col = col * (1 - 0.6 * br) + br * 0.6 * np.array([70.0, 50, 40])
            col = col * (1 - ln) + ln * np.array([15.0, 12, 12])
            img[sy[i], sx[i]] = col
    Image.fromarray(np.clip(img, 0, 255).astype(np.uint8)).save(path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("obj")
    ap.add_argument("--preview")
    a = ap.parse_args()
    V, uv, tris = load_obj(a.obj)
    P, rgba = build(V, uv, tris)
    os.makedirs(os.path.join(ROOT, "data", "makeup"), exist_ok=True)
    Image.fromarray((rgba * 255 + 0.5).astype(np.uint8), "RGBA").save(os.path.join(ROOT, "data", "makeup", "face-paint.png"),
                                                                      optimize=True)
    write_header(V, uv, tris, os.path.join(ROOT, "src", "face-mesh-data.h"))
    if a.preview:
        preview(P, rgba, a.preview)
    print("wrote data/makeup/face-paint.png and src/face-mesh-data.h")


if __name__ == "__main__":
    sys.exit(main())
