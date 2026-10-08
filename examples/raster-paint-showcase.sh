#!/bin/bash
# Builds examples/raster-paint-showcase.pen with the pentool CLI only: `bash examples/raster-paint-showcase.sh [OUT.pen]`.
set -e
P=${P:-pentool}
D=${1:-examples/raster-paint-showcase.pen}
rm -rf "$D" "$(dirname "$D")/.pentool/history/$(basename "$D")"
q() { "$P" "$@" >/dev/null; }
q new "$D" --width 960 --height 600
q canvas "$D" --background '#0b1026'
R() { q raster "$D" "$@"; }

# ---- sky: wide soft strokes stacked into a dusk gradient
R add sky --width 960 --height 600
R fill sky --x 5 --y 5 --color '#16224a'
for band in "40 #1f2f63" "130 #3a3f7a" "220 #6b4a8c" "300 #b4587a" "370 #e8825f" "430 #f6b36b"; do
  set -- $band
  R stroke sky --samples "[[0,$1],[960,$1]]" --brush '{"kind":"soft-round","size":170,"hardness":0,"opacity":0.9}' --color "$2"
done
# stars: pixel brush with scatter, then twinkle with a soft dab
for star in "60 40" "240 90" "410 30" "560 70" "700 25" "860 80" "130 150" "330 170" "520 140" "780 160" "900 200"; do
  set -- $star
  R stroke sky --samples "[[$1,$2]]" --brush '{"kind":"pixel","size":3}' --color '#fff7d6'
done
R stroke sky --samples '[[240,90]]' --brush '{"kind":"soft-round","size":26,"hardness":0,"opacity":0.5}' --color '#fff7d6'
R stroke sky --samples '[[700,25]]' --brush '{"kind":"soft-round","size":26,"hardness":0,"opacity":0.5}' --color '#fff7d6'

# ---- sun: glow, disc, then a haze pass with the blur tool
R stroke sky --samples '[[480,400]]' --brush '{"kind":"soft-round","size":560,"hardness":0,"opacity":0.7}' --color '#ffb25a'
R stroke sky --samples '[[480,400]]' --brush '{"kind":"hard-round","size":150,"hardness":1}' --color '#ffe29a'
R stroke sky --samples '[[0,330],[960,330]]' --brush '{"kind":"soft-round","size":120,"hardness":0.2,"strength":0.6}' --blend blur

# ---- far ridge: lasso selection + fill
R add ridge --width 960 --height 600
R select-lasso ridge --points '[[0,420],[90,360],[170,395],[260,330],[360,400],[450,370],[560,410],[660,345],[760,395],[850,350],[960,405],[960,600],[0,600]]'
R fill ridge --x 5 --y 5 --color '#4a3566'
R select-clear ridge
R stroke ridge --samples '[[0,430],[960,430]]' --brush '{"kind":"soft-round","size":80,"hardness":0,"strength":0.5}' --blend blur

# ---- lake: ellipse-free rect marquee, feathered shoreline
R add lake --width 960 --height 600
R select-marquee lake --rect 0 440 960 160 --feather 6
R fill lake --x 5 --y 5 --color '#1b2b55'
R select-clear lake
# sun reflection: erase streaks through the water with a calligraphic brush
for y in 455 470 488 508 530; do
  w=$((30 + (y - 440)))
  R stroke lake --samples "[[$((480 - w)),$y],[$((480 + w)),$y]]" --brush '{"kind":"calligraphic","size":16,"angle":0,"roundness":0.3,"opacity":0.85}' --color '#ffcf86'
done

# ---- pines near shore: paint one tree, then clone-stamp it with rotation & scale
R add trees --width 960 --height 600
R select-lasso trees --points '[[120,330],[160,420],[135,420],[175,500],[120,500],[120,540],[110,540],[110,500],[55,500],[95,420],[70,420]]'
R fill trees --x 5 --y 5 --color '#0d1b2a'
R select-clear trees
R clone-source trees --x 115 --y 430
R clone-stroke trees --samples '[[115,430]]' --brush '{"kind":"hard-round","size":260,"hardness":1}' --aligned --seed 1
R clone-source trees --x 115 --y 430
R clone-stroke trees --samples '[[760,445]]' --brush '{"kind":"hard-round","size":260,"hardness":1}' --scale 1.25 --seed 2
R clone-source trees --x 115 --y 430
R clone-stroke trees --samples '[[840,455]]' --brush '{"kind":"hard-round","size":260,"hardness":1}' --scale 0.8 --seed 3

# ---- foreground ground: lasso fill, then grass blades with pressure-tapered strokes
R select-lasso trees --points '[[0,560],[160,548],[330,556],[520,545],[700,555],[860,546],[960,552],[960,600],[0,600]]'
R fill trees --x 5 --y 590 --color '#07101c'
R select-clear trees
for x in 20 70 130 200 260 330 400 470 540 610 680 750 820 890 940; do
  R stroke trees --samples "[[$x,556,0.9],[$((x+4)),545,0.5],[$((x+9)),534,0.1]]" --brush '{"kind":"hard-round","size":7,"hardness":1,"pressure_size":true,"min_size":0.1}' --color '#07101c'
done
# a firefly cloud: scattered soft dabs
R stroke trees --samples '[[300,500],[330,480],[620,510]]' --brush '{"kind":"soft-round","size":10,"hardness":0,"scatter":2,"spacing":1.5}' --color '#d8ff8a' --seed 11
# an accidental dab on the sky, repaired with spot healing
R stroke sky --samples '[[600,150]]' --brush '{"kind":"hard-round","size":18,"hardness":1}' --color '#ff00aa'
R heal-spot sky --x 600 --y 150 --radius 14

# ---- title (vector text stays editable over the raster paint)
q text "$D" put title --layer layer-1 --content 'Raster paint' --x 480 --y 90 --size 54 --weight 700 --fill '#fff7d6' --align center
q text "$D" put sub --layer layer-1 --content 'pentool 0.10 - deterministic, replayable, one .pen file' --x 480 --y 128 --size 20 --fill '#f6d9b0' --align center
