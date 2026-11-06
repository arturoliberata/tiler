# Generates assets/setup.svg and assets/logo.svg for the tiler README.
FONT = "font-family=\"'Segoe UI', system-ui, -apple-system, Helvetica, Arial, sans-serif\""

def window(x, y, w, h, body, bar, title_w=None, lines=3, shadow=True, badge=None):
    s = []
    if shadow:
        s.append(f'<rect x="{x+3}" y="{y+5}" width="{w}" height="{h}" rx="9" fill="#000" opacity="0.28"/>')
    s.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="9" fill="{body}"/>')
    s.append(f'<path d="M{x} {y+9} a9 9 0 0 1 9 -9 h{w-18} a9 9 0 0 1 9 9 v15 h-{w} z" fill="{bar}"/>')
    for i, c in enumerate(["#ff5f57", "#febc2e", "#28c840"]):
        s.append(f'<circle cx="{x+14+i*13}" cy="{y+12}" r="4" fill="{c}" opacity="0.9"/>')
    tw = title_w or min(110, w * 0.35)
    s.append(f'<rect x="{x+58}" y="{y+9}" width="{tw:.0f}" height="6" rx="3" fill="#fff" opacity="0.35"/>')
    lw = [0.62, 0.78, 0.45, 0.7, 0.5]
    for i in range(lines):
        ly = y + 42 + i * 18
        if ly + 8 > y + h - 10:
            break
        s.append(f'<rect x="{x+16}" y="{ly}" width="{(w-32)*lw[i%5]:.0f}" height="8" rx="4" fill="#fff" opacity="0.28"/>')
    if badge:
        bw = 112
        s.append(f'<rect x="{x+w-bw-14}" y="{y+h-40}" width="{bw}" height="26" rx="13" fill="#0b1220" opacity="0.55"/>')
        s.append(f'<text x="{x+w-bw/2-14}" y="{y+h-22}" text-anchor="middle" font-size="13" font-weight="600" fill="#fff" {FONT}>{badge}</text>')
    return "\n".join(s)

def setup_svg():
    W, H = 1200, 560
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}">',
           '<defs><linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#16213a"/><stop offset="1" stop-color="#0b1220"/></linearGradient>'
           '<linearGradient id="scr" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#1b2a4a"/><stop offset="1" stop-color="#0f1a30"/></linearGradient></defs>',
           f'<rect width="{W}" height="{H}" rx="28" fill="url(#bg)"/>']
    # external monitor
    fx, fy, fw, fh = 60, 56, 660, 392
    out.append(f'<rect x="{fx}" y="{fy}" width="{fw}" height="{fh}" rx="18" fill="#2a3650"/>')
    sx, sy, sw, sh = fx + 14, fy + 14, fw - 28, fh - 28
    out.append(f'<rect x="{sx}" y="{sy}" width="{sw}" height="{sh}" rx="8" fill="url(#scr)"/>')
    g = 10
    ax, ay, aw, ah = sx + g, sy + g, sw - 2 * g, sh - 2 * g
    lw_ = (aw - g) // 2
    rh = (ah - g) // 2
    out.append(window(ax, ay, lw_, ah, "#2563eb", "#3b82f6", lines=10))
    out.append(window(ax + lw_ + g, ay, aw - lw_ - g, rh, "#0d9488", "#14b8a6", lines=4))
    out.append(window(ax + lw_ + g, ay + rh + g, aw - lw_ - g, ah - rh - g, "#d97706", "#f59e0b", lines=4))
    # stand
    cx = fx + fw / 2
    out.append(f'<path d="M{cx-26} {fy+fh} h52 l8 52 h-68 z" fill="#222c42"/>')
    out.append(f'<rect x="{cx-100}" y="{fy+fh+50}" width="200" height="14" rx="7" fill="#2a3650"/>')
    # laptop
    lx, ly, lw, lh = 770, 140, 380, 250
    out.append(f'<rect x="{lx}" y="{ly}" width="{lw}" height="{lh}" rx="14" fill="#2a3650"/>')
    psx, psy, psw, psh = lx + 12, ly + 12, lw - 24, lh - 24
    out.append(f'<rect x="{psx}" y="{psy}" width="{psw}" height="{psh}" rx="6" fill="url(#scr)"/>')
    out.append(f'<path d="M{lx-34} {ly+lh+2} h{lw+68} l-14 22 a6 6 0 0 1 -5 3 h{-(lw+40)} a6 6 0 0 1 -5 -3 z" fill="#2f3c58"/>')
    out.append(f'<rect x="{lx+lw/2-40}" y="{ly+lh+2}" width="80" height="6" rx="3" fill="#222c42"/>')
    # 4 stacked full-screen windows (cascade shows the stack)
    cols = [("#7c3aed", "#8b5cf6"), ("#e11d48", "#f43f5e"), ("#0284c7", "#0ea5e9"), ("#059669", "#10b981")]
    ww, wh = psw - 16 - 3 * 7, psh - 16 - 3 * 7
    for k, (body, bar) in enumerate(cols):
        off = (3 - k) * 7
        out.append(window(psx + 8 + off, psy + 8 + (3 - k) * 0 + (k) * 7 - 0, ww, wh, body, bar,
                          lines=6, badge=None))
    out.append('</svg>')
    return "\n".join(out)

def logo_svg():
    # Rounded square with the signature 3-tile layout; the right-bottom tile hints at a stack.
    s = ['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" height="256">',
         '<defs><linearGradient id="lb" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#1e2a47"/><stop offset="1" stop-color="#0b1220"/></linearGradient></defs>',
         '<rect x="8" y="8" width="240" height="240" rx="56" fill="url(#lb)"/>',
         # main tile
         '<rect x="44" y="44" width="80" height="168" rx="18" fill="#3b82f6"/>',
         # top-right
         '<rect x="136" y="44" width="76" height="78" rx="18" fill="#14b8a6"/>',
         # bottom-right: stack of two
         '<rect x="148" y="146" width="76" height="78" rx="18" fill="#fde68a"/>',
         '<rect x="136" y="134" width="76" height="78" rx="18" fill="#f59e0b"/>',
         '</svg>']
    return "\n".join(s)

def wordmark_svg():
    s = ['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 340 160" width="340" height="160">',
         '<defs><linearGradient id="lb" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#1e2a47"/><stop offset="1" stop-color="#0b1220"/></linearGradient></defs>',
         '<g transform="translate(8 8) scale(0.5625)">',
         '<rect x="8" y="8" width="240" height="240" rx="56" fill="url(#lb)"/>',
         '<rect x="44" y="44" width="80" height="168" rx="18" fill="#3b82f6"/>',
         '<rect x="136" y="44" width="76" height="78" rx="18" fill="#14b8a6"/>',
         '<rect x="148" y="146" width="76" height="78" rx="18" fill="#fde68a"/>',
         '<rect x="136" y="134" width="76" height="78" rx="18" fill="#f59e0b"/>',
         '</g>',
         f'<text x="176" y="104" font-size="84" font-weight="700" letter-spacing="-2" fill="#3b82f6" {FONT}>tiler</text>',
         '</svg>']
    return "\n".join(s)

import pathlib
d = pathlib.Path("assets")
(d / "setup.svg").write_text(setup_svg())
(d / "logo.svg").write_text(logo_svg())
(d / "logo-wordmark.svg").write_text(wordmark_svg())
