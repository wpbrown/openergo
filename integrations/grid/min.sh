#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
input_file="$script_dir/system-setup.lua"
output_file="$script_dir/system-setup.min.lua"

sed \
	-e 's/\<led_animation_phase_rate_type\>/glpfs/g' \
	-e 's/\<potmeter_value\>/pva/g' \
	-e 's/\<midi_send\>/gms/g' \
	-e 's/\<led_color\>/glc/g' \
	-e 's/\<led_value\>/glp/g' \
	"$input_file" >"$output_file"

printf 'Wrote %s\n' "$output_file"
