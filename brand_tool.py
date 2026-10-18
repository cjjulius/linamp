import sys, os, zipfile, shutil
from PIL import Image, ImageDraw


MINH = 3  # letters are this tall; grip-bar highlights are only 1-2px


def is_white(p):
    return p[0] > 150 and p[1] > 150 and p[2] > 120


def col_count(px, x, y0, y1):
    return sum(1 for y in range(y0, y1) if is_white(px[x, y]))


def letters_in(img, x0, x1, y0, y1):
    """Segment letter columns (tall white runs) into (cx0, cx1) ranges."""
    px = img.load()
    cols = []
    inl = False
    start = 0
    for x in range(x0, x1):
        has = col_count(px, x, y0, y1) >= MINH
        if has and not inl:
            inl = True
            start = x
        elif not has and inl:
            inl = False
            cols.append((start, x - 1))
    if inl:
        cols.append((start, x1 - 1))
    return cols


def ink_bbox(img, x0, x1, y0, y1):
    px = img.load()
    minx = x1
    maxx = x0 - 1
    miny = y1
    maxy = y0 - 1
    for x in range(x0, x1):
        if col_count(px, x, y0, y1) < MINH:
            continue
        for y in range(y0, y1):
            if is_white(px[x, y]):
                minx = min(minx, x)
                maxx = max(maxx, x)
                miny = min(miny, y)
                maxy = max(maxy, y)
    if maxx < x0:
        return None
    return (minx, miny, maxx, maxy)


def mask_from(crop, bg, tol=42):
    """Alpha mask: pixels differing from bg (letter ink + bevel) are opaque."""
    crop = crop.convert("RGB")
    m = Image.new("L", crop.size, 0)
    mp = m.load()
    cp = crop.load()
    for y in range(crop.height):
        for x in range(crop.width):
            p = cp[x, y]
            if abs(p[0] - bg[0]) + abs(p[1] - bg[1]) + abs(p[2] - bg[2]) > tol:
                mp[x, y] = 255
    return m


def brightest(crop):
    cp = crop.convert("RGB").load()
    best = (255, 255, 255)
    bl = -1
    for y in range(crop.height):
        for x in range(crop.width):
            p = cp[x, y]
            s = p[0] + p[1] + p[2]
            if s > bl:
                bl = s
                best = p
    return best


def brand_region(img, sx0, sx1, sy0, sy1):
    """Replace the first letter (W) of a 'WINAMP...' wordmark with a synthesized
    'L' matched to the font's letter box, re-centering the word."""
    bb = ink_bbox(img, sx0, sx1, sy0, sy1)
    if bb is None:
        return
    # Narrow letter groups (exclude the wide gold grip bars).
    groups = [c for c in letters_in(img, sx0, sx1, bb[1], bb[3] + 1) if (c[1] - c[0] + 1) <= 15]
    if len(groups) < 2:
        return
    # Cluster groups separated by big gaps; the wordmark is the first wide,
    # multi-letter cluster (this skips stray buttons and grip-bar caps).
    clusters = [[groups[0]]]
    for g in groups[1:]:
        if g[0] - clusters[-1][-1][1] > 6:
            clusters.append([g])
        else:
            clusters[-1].append(g)
    cols = None
    for cl in clusters:
        span = cl[-1][1] - cl[0][0] + 1
        if len(cl) >= 4 and span >= 20:
            cols = cl
            break
    if cols is None or len(cols) < 2:
        return
    lx0, lx1 = cols[0][0], cols[-1][1]

    px = img.load()
    # Actual letter vertical extent within the word's x-range.
    miny, maxy = bb[3], bb[1]
    for x in range(lx0, lx1 + 1):
        for y in range(sy0, sy1):
            if is_white(px[x, y]):
                miny = min(miny, y)
                maxy = max(maxy, y)
    if maxy < miny:
        return

    w_x1 = cols[0][1]                    # right edge of the W
    i_x0 = cols[1][0]                    # left edge of the I
    gap = max(1, i_x0 - w_x1 - 1)        # W→I spacing to reuse
    minx, maxx = lx0, lx1

    sy = max(sy0, miny - 1)
    bg = tuple(px[(minx + maxx) // 2, sy][:3])

    # "INAMP..." stays as real font art (start exactly at the I).
    rest = img.crop((i_x0, miny, maxx + 1, maxy + 1))
    rest_mask = mask_from(rest, bg)
    white = brightest(rest)

    # Synthesize an L matched to the letter height/weight.
    target_h = maxy - miny + 1
    vsw = max(1, round(target_h * 0.30))
    lw = max(vsw + 2, round(target_h * 0.58))
    Lg = Image.new("RGB", (lw, target_h), bg)
    dr = ImageDraw.Draw(Lg)
    dr.rectangle([0, 0, vsw - 1, target_h - 1], fill=white)             # stem
    dr.rectangle([0, target_h - vsw, lw - 1, target_h - 1], fill=white) # foot
    L_mask = mask_from(Lg, bg)

    # Erase the old wordmark (a few px extra left to catch the W's thin
    # diagonal/bevel), then re-lay "L" + rest, centered in the bar.
    ex0 = max(sx0, minx - 5)
    ey0 = max(sy0, miny - 1)
    ey1 = min(sy1, maxy + 2)
    for x in range(ex0, maxx + 1):
        for y in range(ey0, ey1):
            img.putpixel((x, y), bg)
    total_w = lw + gap + rest.width
    nx0 = (sx0 + sx1) // 2 - total_w // 2
    img.paste(Lg, (nx0, miny), L_mask)
    img.paste(rest, (nx0 + lw + gap, miny), rest_mask)


def main():
    base_wsz, outdir, out_wsz = sys.argv[2], sys.argv[3], sys.argv[4]
    if os.path.exists(outdir):
        shutil.rmtree(outdir)
    os.makedirs(outdir)
    with zipfile.ZipFile(base_wsz) as z:
        z.extractall(outdir)

    # find bmp paths case-insensitively
    def find(name):
        for f in os.listdir(outdir):
            if f.lower() == name:
                return os.path.join(outdir, f)
        return None

    pl = Image.open(find("pledit.bmp")).convert("RGB")
    tb = Image.open(find("titlebar.bmp")).convert("RGB")
    eq = Image.open(find("eqmain.bmp")).convert("RGB")

    # Main title bar: selected (27,0,275,14) and unselected (27,15,...).
    brand_region(tb, 100, 260, 1, 13)
    brand_region(tb, 100, 260, 16, 28)
    # EQ title bar: selected y134..148, unselected y149..163.
    brand_region(eq, 60, 240, 135, 147)
    brand_region(eq, 60, 240, 150, 162)
    # Playlist title bar: selected y0..20, unselected y21..41.
    brand_region(pl, 10, 205, 1, 19)
    brand_region(pl, 10, 205, 22, 40)

    pl.save(find("pledit.bmp"))
    tb.save(find("titlebar.bmp"))
    eq.save(find("eqmain.bmp"))

    # Re-zip into the branded skin.
    if os.path.exists(out_wsz):
        os.remove(out_wsz)
    with zipfile.ZipFile(out_wsz, "w", zipfile.ZIP_DEFLATED) as z:
        for f in os.listdir(outdir):
            z.write(os.path.join(outdir, f), f)
    print("branded ->", out_wsz)


if __name__ == "__main__":
    main()
