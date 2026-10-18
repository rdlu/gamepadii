#!/usr/bin/env bash
# Makes the README's demos again (`mise run demos`): each tape with VHS,
# then a GIF and a still image for each one. Needs vhs, ffmpeg and the
# 0xProto Nerd Font. The pad is simulated from the test fixtures.
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release --quiet
rm -rf /tmp/gamepadii-demo
media=../media
mkdir -p "$media"
for tape in cli tui; do
  vhs "$tape.tape"
  ffmpeg -loglevel error -y -i "$tape.webm" -vf "fps=8,scale=1000:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=32:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" "$media/$tape.gif"
done
du -ch "$media"/* | tail -n 1
