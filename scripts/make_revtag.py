#!/usr/bin/env python3
"""Regenerates docs/support/revtag.svg (donation card with QR code).
Needs: pip install qrcode. Usage: scripts/make_revtag.py docs/support/revtag.svg"""
import qrcode, sys
URL = "https://revolut.me/gama87"
q = qrcode.QRCode(border=0, error_correction=qrcode.constants.ERROR_CORRECT_M)
q.add_data(URL); q.make(fit=True)
m = q.get_matrix(); n = len(m)
W, H = 420, 700
qs = 230                      # QR size
cell = qs / n
qx, qy = (W - qs) / 2, 330
path = "".join(f"M{qx+x*cell:.2f} {qy+y*cell:.2f}h{cell:.2f}v{cell:.2f}h-{cell:.2f}z" for y in range(n) for x in range(n) if m[y][x])
F = "font-family=\"'Segoe UI','Helvetica Neue',Helvetica,Arial,sans-serif\""
svg = f'''<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" role="img" aria-label="Support Gall: Revolut @gama87, {URL}">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#6f8e72"/><stop offset="1" stop-color="#4b6550"/></linearGradient>
    <linearGradient id="rb" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#b8f5c8"/><stop offset=".3" stop-color="#a8e8f0"/><stop offset=".55" stop-color="#c3b5f7"/>
      <stop offset=".8" stop-color="#f2b8d8"/><stop offset="1" stop-color="#f7e3a8"/></linearGradient>
  </defs>
  <rect width="{W}" height="{H}" rx="28" fill="url(#bg)"/>
  <circle cx="{W/2}" cy="88" r="46" fill="#fff"/>
  <g transform="translate({W/2-24} 62)" fill="none" stroke="#4b6550" stroke-width="4" stroke-linecap="round" stroke-linejoin="round">
    <path d="M10 2c-3 4 3 6 0 10M20 2c-3 4 3 6 0 10M30 2c-3 4 3 6 0 10" stroke-width="3"/>
    <path d="M2 18h38v14a14 14 0 0 1-14 14h-10A14 14 0 0 1 2 32z" fill="#4b6550"/>
    <path d="M40 22h4a7 7 0 0 1 0 14h-5"/>
  </g>
  <text x="{W/2}" y="172" text-anchor="middle" fill="#fff" font-size="20" font-weight="600" {F}>Buy me a coffee · Revolut</text>
  <text x="{W/2}" y="236" text-anchor="middle" fill="#fff" font-size="54" font-weight="800" {F}>@gama87</text>
  <text x="{W/2}" y="274" text-anchor="middle" fill="#fff" font-size="17" font-weight="600" letter-spacing="2" {F}>REVTAG</text>
  <rect x="{qx-36}" y="{qy-36}" width="{qs+72}" height="{qs+72}" rx="26" fill="url(#rb)"/>
  <rect x="{qx-26}" y="{qy-26}" width="{qs+52}" height="{qs+52}" rx="18" fill="#fff"/>
  <path d="{path}" fill="#000"/>
  <text x="{W/2}" y="{qy+qs+80}" text-anchor="middle" fill="#fff" font-size="20" font-weight="600" {F}>revolut.me/gama87</text>
  <text x="{W/2}" y="{qy+qs+112}" text-anchor="middle" fill="#e6efe7" font-size="14" {F}>Thank you for supporting Gall ♥</text>
</svg>
'''
open(sys.argv[1], "w").write(svg)
