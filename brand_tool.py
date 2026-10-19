import sys, os, zipfile, shutil
from PIL import Image

MINH = 3  # letters are this tall; grip-bar highlights are only 1-2px


def is_white(p):
    return p[0] > 150 and p[1] > 150 and p[2] > 120


def col_count(px, x, y0, y1):
    return sum(1 for y in range(y0, y1) if is_white(px[x, y]))


def letters_in(img, x0, x1, y0, y1):
    px = img.load()
    cols = []
    inl = False
    start = 0
    for x in range(x0, x1):
        has = col_count(px, x, y0, y1) >= MINH
        if has and not inl:
            inl, start = True, x
        elif not has and inl:
            inl = False
            cols.append((start, x - 1))
    if inl:
        cols.append((start, x1 - 1))
    return cols


def ink_bbox(img, x0, x1, y0, y1):
    px = img.load()
    minx, maxx, miny, maxy = x1, x0 - 1, y1, y0 - 1
    for x in range(x0, x1):
        if col_count(px, x, y0, y1) < MINH:
            continue
        for y in range(y0, y1):
            if is_white(px[x, y]):
                minx, maxx = min(minx, x), max(maxx, x)
                miny, maxy = min(miny, y), max(maxy, y)
    return None if maxx < x0 else (minx, miny, maxx, maxy)


def wordmark(img, sx0, sx1, sy0, sy1):
    """Return (cols, miny, maxy) for the first wide multi-letter cluster."""
    bb = ink_bbox(img, sx0, sx1, sy0, sy1)
    if bb is None:
        return None
    groups = [c for c in letters_in(img, sx0, sx1, bb[1], bb[3] + 1) if (c[1] - c[0] + 1) <= 15]
    if len(groups) < 2:
        return None
    clusters = [[groups[0]]]
    for g in groups[1:]:
        (clusters.append([g]) if g[0] - clusters[-1][-1][1] > 6 else clusters[-1].append(g))
    cols = None
    for cl in clusters:
        if len(cl) >= 4 and (cl[-1][1] - cl[0][0] + 1) >= 20:
            cols = cl
            break
    if cols is None:
        return None
    # vertical extent of the cluster's letters
    px = img.load()
    miny, maxy = sy1, sy0
    for x in range(cols[0][0], cols[-1][1] + 1):
        for y in range(sy0, sy1):
            if is_white(px[x, y]):
                miny, maxy = min(miny, y), max(maxy, y)
    return (cols, miny, maxy)


def white_yextent(img, x0, x1, y0, y1):
    px = img.load()
    ys = [y for y in range(y0, y1 + 1) if any(is_white(px[x, y]) for x in range(x0, x1 + 1))]
    return (ys[0], ys[-1]) if ys else None


def is_L(img, x0, x1, ly0, ly1):
    """L: full-height left stem, wide bottom row, empty top-right quadrant."""
    px = img.load()
    w, h = x1 - x0 + 1, ly1 - ly0 + 1
    if w < 2 or h < 3 or w > h * 1.3:
        return False
    left = sum(is_white(px[x0, y]) for y in range(ly0, ly1 + 1)) / h
    bottom = sum(is_white(px[x, ly1]) for x in range(x0, x1 + 1)) / w
    tr_rows = range(ly0, ly0 + max(1, h // 2))
    tr_cols = range(max(x0, x1 - 1), x1 + 1)
    tr_ink = sum(is_white(px[cx, cy]) for cy in tr_rows for cx in tr_cols)
    tr_ratio = tr_ink / max(1, len(list(tr_rows)) * 2)
    return left > 0.7 and bottom > 0.6 and tr_ratio < 0.2


def extract_L(img, sx0, sx1, sy0, sy1):
    # Scan ALL letters in the region (both words) for an L-shaped glyph.
    bb = ink_bbox(img, sx0, sx1, sy0, sy1)
    if bb is None:
        return None
    miny, maxy = bb[1], bb[3]
    groups = [c for c in letters_in(img, sx0, sx1, miny, maxy + 1) if (c[1] - c[0] + 1) <= 15]
    for cx0, cx1 in groups:
        ye = white_yextent(img, cx0, cx1, miny, maxy)
        if ye and is_L(img, cx0, cx1, ye[0], ye[1]):
            return img.crop((cx0, ye[0], cx1 + 1, ye[1] + 1))
    return None


def mask_from(crop, bg, tol=40):
    crop = crop.convert("RGB")
    m = Image.new("L", crop.size, 0)
    mp, cp = m.load(), crop.load()
    for y in range(crop.height):
        for x in range(crop.width):
            p = cp[x, y]
            if abs(p[0] - bg[0]) + abs(p[1] - bg[1]) + abs(p[2] - bg[2]) > tol:
                mp[x, y] = 255
    return m


def brand_region(img, L_glyph, sx0, sx1, sy0, sy1):
    if L_glyph is None:
        return
    res = wordmark(img, sx0, sx1, sy0, sy1)
    if res is None:
        return
    cols, miny, maxy = res
    lx0, lx1 = cols[0][0], cols[-1][1]
    w_x1 = cols[0][1]
    i_x0 = cols[1][0]
    gap = max(1, i_x0 - w_x1 - 1)
    h = maxy - miny + 1

    px = img.load()
    bg = tuple(px[(lx0 + lx1) // 2, max(sy0, miny - 1)][:3])

    # Keep the real "INAMP…" art, including its drop-shadow row below the baseline.
    rest = img.crop((i_x0, miny, lx1 + 1, min(sy1, maxy + 2)))
    rest_mask = mask_from(rest, bg)

    Lg = L_glyph.convert("RGB")
    if abs(Lg.height - h) > 1:
        Lg = Lg.resize((max(1, round(Lg.width * h / Lg.height)), h), Image.NEAREST)
    lbg = tuple(Lg.getpixel((Lg.width - 1, 0)))  # glyph's own top-right bg
    L_mask = mask_from(Lg, lbg)
    Lw = Lg.width

    # Erase the original wordmark (extra left for the W's thin diagonal).
    ex0 = max(sx0, lx0 - 5)
    ey0 = max(sy0, miny - 1)
    ey1 = min(sy1, maxy + 2)
    for x in range(ex0, lx1 + 1):
        for y in range(ey0, ey1):
            img.putpixel((x, y), bg)

    # Re-lay "L" + rest, centered on the ORIGINAL wordmark centre, bottom-aligned.
    oc = (lx0 + lx1) // 2
    total = Lw + gap + rest.width
    nx0 = oc - total // 2
    img.paste(Lg, (nx0, maxy - Lg.height + 1), L_mask)
    img.paste(rest, (nx0 + Lw + gap, miny), rest_mask)


def main():
    base_wsz, outdir, out_wsz = sys.argv[2], sys.argv[3], sys.argv[4]
    if os.path.exists(outdir):
        shutil.rmtree(outdir)
    os.makedirs(outdir)
    with zipfile.ZipFile(base_wsz) as z:
        z.extractall(outdir)

    def find(name):
        for f in os.listdir(outdir):
            if f.lower() == name:
                return os.path.join(outdir, f)
        return None

    pl = Image.open(find("pledit.bmp")).convert("RGB")
    tb = Image.open(find("titlebar.bmp")).convert("RGB")
    eq = Image.open(find("eqmain.bmp")).convert("RGB")

    # Real "L" glyphs lifted from the font: big one from EQUALIZER (for main+EQ),
    # small one from PLAYLIST (for the playlist).
    L_big = extract_L(eq, 60, 240, 135, 147)
    L_small = extract_L(pl, 10, 205, 1, 19)
    if L_big is None:
        L_big = L_small
    if L_small is None:
        L_small = L_big
    print("L_big:", None if L_big is None else L_big.size,
          "L_small:", None if L_small is None else L_small.size)

    brand_region(tb, L_big, 100, 260, 1, 13)
    brand_region(tb, L_big, 100, 260, 16, 28)
    brand_region(eq, L_big, 60, 240, 135, 147)
    brand_region(eq, L_big, 60, 240, 150, 162)
    brand_region(pl, L_small, 10, 205, 1, 19)
    brand_region(pl, L_small, 10, 205, 22, 40)

    pl.save(find("pledit.bmp"))
    tb.save(find("titlebar.bmp"))
    eq.save(find("eqmain.bmp"))

    if os.path.exists(out_wsz):
        os.remove(out_wsz)
    with zipfile.ZipFile(out_wsz, "w", zipfile.ZIP_DEFLATED) as z:
        for f in os.listdir(outdir):
            z.write(os.path.join(outdir, f), f)
    print("branded ->", out_wsz)


if __name__ == "__main__":
    main()
