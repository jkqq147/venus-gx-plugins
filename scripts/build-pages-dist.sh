#!/usr/bin/env bash
set -euo pipefail

output="${1:-dist/pages}"
assets_file="$output/.assets.tsv"

rm -rf "$output"
mkdir -p "$output/catalog"
cp catalog/plugins.json "$output/catalog/plugins.json"
cp infra/cloudflare/_headers "$output/_headers"

gh api --paginate "repos/${GITHUB_REPOSITORY:-jkqq147/venus-gx-plugins}/releases?per_page=100" \
	--jq '.[] | select(.draft == false) | .tag_name as $tag | .assets[] | select(.name == "venus-plugin-manager-armv7" or (.name | endswith(".vplugin"))) | [$tag, .name, .url] | @tsv' \
	> "$assets_file"

while IFS=$'\t' read -r tag name asset_url; do
	[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || continue
	[[ "$name" != */* ]] || {
		echo "invalid release asset name: $name" >&2
		exit 1
	}

	public_name="$name"
	if [[ "$name" == "venus-plugin-manager-armv7" ]]; then
		public_name="$name.bin"
	fi
	destination="$output/releases/download/$tag/$public_name"
	mkdir -p "$(dirname "$destination")"
	gh api "$asset_url" -H "Accept: application/octet-stream" > "$destination.tmp"
	mv "$destination.tmp" "$destination"
done < "$assets_file"

rm "$assets_file"
