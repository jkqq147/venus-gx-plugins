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
	--jq '.[] | select(.draft == false) | .tag_name as $tag | .assets[] | select(.name == "venus-plugin-manager-armv7" or .name == "venus-plugin-manager-armv7.bin" or (.name | endswith(".vplugin"))) | [$tag, .name, .url] | @tsv' \
	> "$assets_file"

valid_release_tag() {
	[[ "$1" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ || "$1" =~ ^[a-z0-9]+(-[a-z0-9]+)*-v[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

sha256_file() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | awk '{print $1}'
	else
		shasum -a 256 "$1" | awk '{print $1}'
	fi
}

while IFS=$'\t' read -r tag name asset_url; do
	valid_release_tag "$tag" || continue
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
actual_sha256="$(sha256_file "$manager_path")"
[[ "$actual_sha256" == "$manager_sha256" ]] || {
	echo "manager release SHA-256 mismatch" >&2
	exit 1
}

while IFS=$'\t' read -r plugin_id plugin_version plugin_url plugin_sha256; do
	url_prefix="https://venus-gx-plugins.pages.dev/releases/download/"
	[[ "$plugin_url" == "$url_prefix"* ]] || {
		echo "catalog URL is outside the distribution origin: $plugin_url" >&2
		exit 1
	}
	relative="${plugin_url#https://venus-gx-plugins.pages.dev/}"
	IFS=/ read -r releases download tag name extra <<< "$relative"
	[[ "$releases" == "releases" && "$download" == "download" && -z "$extra" ]] || {
		echo "invalid catalog asset URL: $plugin_url" >&2
		exit 1
	}
	valid_release_tag "$tag" || {
		echo "invalid catalog release tag: $tag" >&2
		exit 1
	}
	[[ "$name" == "$plugin_id-$plugin_version.vplugin" ]] || {
		echo "catalog asset name does not match $plugin_id $plugin_version: $name" >&2
		exit 1
	}
	plugin_path="$output/$relative"
	[[ -f "$plugin_path" ]] || {
		echo "catalog release asset is missing: $plugin_path" >&2
		exit 1
	}
	actual_sha256="$(sha256_file "$plugin_path")"
	[[ "$actual_sha256" == "$plugin_sha256" ]] || {
		echo "catalog release asset SHA-256 mismatch: $plugin_id $plugin_version" >&2
		exit 1
	}
done < <(jq -r '.plugins[] | [.id, .version, .package.url, .package.sha256] | @tsv' catalog/plugins.json)
