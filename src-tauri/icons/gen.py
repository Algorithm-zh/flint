import struct, zlib, os, math

OUT = os.path.dirname(os.path.abspath(__file__))

# ---------- tiny PNG writer ----------
def write_png(path, w, h, rgba):
    raw = bytearray()
    stride = w * 4
    for y in range(h):
        raw.append(0)
        raw += rgba[y * stride:(y + 1) * stride]
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)

# ---------- geometry helpers ----------
def point_in_poly(x, y, pts):
    inside = False
    n = len(pts)
    j = n - 1
    for i in range(n):
        xi, yi = pts[i]
        xj, yj = pts[j]
        if ((yi > y) != (yj > y)) and (x < (xj - xi) * (y - yi) / (yj - yi + 1e-12) + xi):
            inside = not inside
        j = i
    return inside

def sd_round_rect(px, py, cx, cy, hw, hh, r):
    qx = abs(px - cx) - (hw - r)
    qy = abs(py - cy) - (hh - r)
    ax = max(qx, 0.0)
    ay = max(qy, 0.0)
    return math.hypot(ax, ay) + min(max(qx, qy), 0.0) - r

def lerp(a, b, t):
    return a + (b - a) * t

BOLT = [(0.600, 0.115), (0.300, 0.552), (0.468, 0.552), (0.392, 0.888),
        (0.706, 0.436), (0.528, 0.436)]

def render(size):
    S = 4  # supersample
    W = size * S
    buf = bytearray(W * W * 4)
    cx = cy = W / 2.0
    hw = hh = W * 0.5 - W * 0.028
    r = W * 0.225
    bolt = [(x * W, y * W) for (x, y) in BOLT]

    for y in range(W):
        for x in range(W):
            px = x + 0.5
            py = y + 0.5
            d = sd_round_rect(px, py, cx, cy, hw, hh, r)
            a = 1.0 if d < 0 else 0.0
            if a == 0.0:
                i = (y * W + x) * 4
                buf[i] = buf[i+1] = buf[i+2] = buf[i+3] = 0
                continue
            ty = (py / W)
            tx = (px / W)
            # base gradient: deep charcoal, slightly lifted at top-left
            base_r = lerp(0x26, 0x10, ty)
            base_g = lerp(0x26, 0x10, ty)
            base_b = lerp(0x2E, 0x15, ty)
            # soft diagonal sheen
            sheen = max(0.0, 1.0 - (abs((tx + ty) - 0.55) / 0.62))
            base_r += sheen * 13
            base_g += sheen * 13
            base_b += sheen * 16
            # inner top highlight line
            rim = 1.0 - min(1.0, abs(d + W * 0.006) / (W * 0.010))
            if rim > 0:
                base_r += rim * 42
                base_g += rim * 44
                base_b += rim * 50
            R, G, B = base_r, base_g, base_b
            if point_in_poly(px, py, bolt):
                t = (py - W * 0.115) / (W * 0.773)
                R = lerp(0xD8, 0x6E, t)
                G = lerp(0xFF, 0xE8, t)
                B = lerp(0x6A, 0x7E, t)
            i = (y * W + x) * 4
            buf[i] = max(0, min(255, int(R)))
            buf[i+1] = max(0, min(255, int(G)))
            buf[i+2] = max(0, min(255, int(B)))
            buf[i+3] = 255

    # downsample S x S -> size
    out = bytearray(size * size * 4)
    n = S * S
    for y in range(size):
        for x in range(size):
            ar = ag = ab = aa = 0
            for yy in range(S):
                base = ((y * S + yy) * W + x * S) * 4
                for xx in range(S):
                    o = base + xx * 4
                    a_ = buf[o+3]
                    ar += buf[o] * a_
                    ag += buf[o+1] * a_
                    ab += buf[o+2] * a_
                    aa += a_
            o = (y * size + x) * 4
            if aa == 0:
                out[o] = out[o+1] = out[o+2] = out[o+3] = 0
            else:
                out[o] = min(255, ar // aa)
                out[o+1] = min(255, ag // aa)
                out[o+2] = min(255, ab // aa)
                out[o+3] = min(255, aa // n)
    return bytes(out)

sizes = {"icon.png": 256, "128x128@2x.png": 256, "128x128.png": 128,
         "32x32.png": 32, "icon_64.png": 64, "icon_48.png": 48}
pngs = {}
for name, s in sizes.items():
    data = render(s)
    write_png(os.path.join(OUT, name), s, s, data)
    pngs[s] = data
    print("wrote", name, s)

# ---------- ICO ----------
# Classic DIB payloads (not PNG-compressed): every consumer on Windows,
# including the resource loader Tauri uses for the tray, understands these.
def ico_dib(rgba, size):
    hdr = struct.pack(
        "<IiiHHIIiiII",
        40, size, size * 2, 1, 32, 0, size * size * 4, 0, 0, 0, 0
    )
    xor = bytearray()
    for y in range(size - 1, -1, -1):  # bottom-up
        row = rgba[y * size * 4:(y + 1) * size * 4]
        bgra = bytearray(len(row))
        bgra[0::4] = row[2::4]
        bgra[1::4] = row[1::4]
        bgra[2::4] = row[0::4]
        bgra[3::4] = row[3::4]
        xor += bgra
    mask_stride = ((size + 31) // 32) * 4
    mask = bytes(mask_stride * size)
    return hdr + bytes(xor) + mask


ico_sizes = [16, 24, 32, 48, 64, 128, 256]
images = []
for s in ico_sizes:
    data = pngs.get(s) or render(s)
    images.append((s, ico_dib(data, s)))

offset = 6 + 16 * len(images)
head = struct.pack("<HHH", 0, 1, len(images))
dirdata = b""
blob = b""
for (s, data) in images:
    d = 0 if s >= 256 else s
    dirdata += bytes([d, d, 0, 0]) + struct.pack("<HHII", 1, 32, len(data), offset)
    offset += len(data)
    blob += data
ico = head + dirdata + blob
open(os.path.join(OUT, "icon.ico"), "wb").write(ico)
print("wrote icon.ico", len(ico), "bytes,", len(images), "images")

