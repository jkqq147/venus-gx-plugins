#!/usr/bin/env bash
set -euo pipefail

output="${1:-dist/pages}"
assets_file="$output/.assets.tsv"

rm -rf "$output"
mkdir -p "$output/catalog" "$output/manager"
cp catalog/plugins.json "$output/catalog/plugins.json"
cp manager/release.json "$output/manager/release.json"
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

manager_version="$(jq -r '.version' manager/release.json)"
manager_url="$(jq -r '.binary.url' manager/release.json)"
manager_sha256="$(jq -r '.binary.sha256' manager/release.json)"
manager_path="$output/releases/download/v$manager_version/venus-plugin-manager-armv7.bin"
expected_url="https://venus-gx-plugins.pages.dev/releases/download/v$manager_version/venus-plugin-manager-armv7.bin"

[[ "$manager_url" == "$expected_url" ]] || {
	echo "manager release URL does not match version $manager_version" >&2
	exit 1
}
[[ -f "$manager_path" ]] || {
	echo "manager release asset is missing: $manager_path" >&2
	exit 1
}
if command -v sha256sum >/dev/null 2>&1; then
	actual_sha256="$(sha256sum "$manager_path" | awk '{print $1}')"
else
	actual_sha256="$(shasum -a 256 "$manager_path" | awk '{print $1}')"
fi
[[ "$actual_sha256" == "$manager_sha256" ]] || {
	echo "manager release SHA-256 mismatch" >&2
	exit 1
}
