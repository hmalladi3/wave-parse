#!/usr/bin/env bash
# Fetch the open WiFi-CSI-MiningTool breathing dataset (subject S10) used to
# validate DSP-VAL-001. Real Linux 802.11n CSI Tool data, BPM-labeled.
# Source: https://github.com/AlbanyArmenta0711/WiFi-CSI-MiningTool (open access)
set -euo pipefail

DEST="$(cd "$(dirname "$0")/.." && pwd)/data/breathing"
BASE="https://raw.githubusercontent.com/AlbanyArmenta0711/WiFi-CSI-MiningTool/main/Datasets/Subjects/Breathing/S10_CSI"
mkdir -p "$DEST"

for bpm in 9 12 15 18 21; do
  echo "fetching ${bpm} BPM..."
  curl -fsSL "$BASE/${bpm}BPMAmp.csv" -o "$DEST/s10_${bpm}bpm_amp.csv"
done

echo "done -> $DEST"
echo "validate: cargo test -p wave-core --test dsp validates_within_two_bpm_on_dataset -- --nocapture"
echo "explore:  cargo run --release --example validate_breathing -- $DEST/s10_15bpm_amp.csv 25.0 15"
