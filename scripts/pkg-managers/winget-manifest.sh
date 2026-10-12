#!/usr/bin/env bash
# ============================================================================
# winget-manifest.sh — 为已发布的 Release 生成 winget 清单（microsoft/winget-pkgs 用）
# ----------------------------------------------------------------------------
# 用法：scripts/pkg-managers/winget-manifest.sh <版本> [输出根目录]
#   例：scripts/pkg-managers/winget-manifest.sh 0.126.0 /tmp/winget
#   产出：<输出根目录>/manifests/h/huanfeng/WindInput/<版本>/ 下四个 yaml
#
# 只在两种场合用它：
#   1. 首次提交（winget-pkgs 里还没有这个包时，CI 的 komac update 无从更新）；
#   2. CI 那条路坏了要手工补提交。
# 日常发版由 release-published.yml 的 winget job 自动提交，它以 winget-pkgs 里的上一版
# 为模板，**不读本脚本**——改了这里的元数据（描述、标签等），只对下一次手工提交生效，
# 已入库的那份要另提 PR 去改。各字段的取值依据见 docs/design/package-managers.md。
# ============================================================================
set -euo pipefail

VERSION="${1:?用法: $0 <版本> [输出根目录]}"
OUT_ROOT="${2:-.}"
VERSION="${VERSION#v}"

REPO="huanfeng/WindInput"
ID="huanfeng.WindInput"
MANIFEST_VERSION="1.10.0"
ASSET="WindInput-Setup-${VERSION}.exe"
URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ASSET}"

# 哈希取 Release 上的 .sha256 sidecar（与 R2 同步、在线升级同一口径），再对实物复核一遍：
# sidecar 与安装包不符说明 Release 资产被动过（比如签名后只换了 exe），宁可失败也不提交错值。
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
gh release download "v${VERSION}" -R "$REPO" -p "$ASSET" -p "$ASSET.sha256" -D "$WORK"
SHA="$(awk '{print toupper($1)}' "$WORK/$ASSET.sha256")"
ACTUAL="$(sha256sum "$WORK/$ASSET" | awk '{print toupper($1)}')"
if [ "$SHA" != "$ACTUAL" ]; then
    echo "ERROR: sidecar($SHA) 与安装包实际哈希($ACTUAL) 不符" >&2
    exit 1
fi
RELEASE_DATE="$(gh release view "v${VERSION}" -R "$REPO" --json publishedAt -q '.publishedAt[0:10]')"

DIR="$OUT_ROOT/manifests/h/huanfeng/WindInput/$VERSION"
mkdir -p "$DIR"
HEADER="# yaml-language-server: \$schema=https://aka.ms/winget-manifest"

cat > "$DIR/$ID.yaml" <<YAML
$HEADER.version.$MANIFEST_VERSION.schema.json

PackageIdentifier: $ID
PackageVersion: $VERSION
DefaultLocale: zh-CN
ManifestType: version
ManifestVersion: $MANIFEST_VERSION
YAML

# ProductCode = ARP 子键名，wind-installer 以 [app] display_name 建键（config/app.toml）。
# winget 靠它把「已安装的那份」与本清单对上号，upgrade / uninstall / list 都依赖它；
# 改 display_name 会让存量安装与清单失联。
cat > "$DIR/$ID.installer.yaml" <<YAML
$HEADER.installer.$MANIFEST_VERSION.schema.json

PackageIdentifier: $ID
PackageVersion: $VERSION
Platform:
- Windows.Desktop
MinimumOSVersion: 10.0.0.0
InstallerType: exe
Scope: machine
InstallModes:
- interactive
- silent
- silentWithProgress
InstallerSwitches:
  Silent: --silent
  SilentWithProgress: --quiet
  InstallLocation: --dir "<INSTALLPATH>"
ExpectedReturnCodes:
- InstallerReturnCode: 1618
  ReturnResponse: installInProgress
- InstallerReturnCode: 3010
  ReturnResponse: rebootRequiredToFinish
UpgradeBehavior: install
ElevationRequirement: elevatesSelf
ProductCode: 清风输入法
ReleaseDate: $RELEASE_DATE
AppsAndFeaturesEntries:
- DisplayName: 清风输入法
  Publisher: 清风输入法 项目
  ProductCode: 清风输入法
Installers:
- Architecture: x64
  InstallerUrl: $URL
  InstallerSha256: $SHA
ManifestType: installer
ManifestVersion: $MANIFEST_VERSION
YAML

cat > "$DIR/$ID.locale.zh-CN.yaml" <<YAML
$HEADER.defaultLocale.$MANIFEST_VERSION.schema.json

PackageIdentifier: $ID
PackageVersion: $VERSION
PackageLocale: zh-CN
Publisher: 清风输入法 项目
PublisherUrl: https://github.com/huanfeng
PublisherSupportUrl: https://github.com/$REPO/issues
Author: huanfeng
PackageName: 清风输入法
PackageUrl: https://windinput.com
License: MIT
LicenseUrl: https://github.com/$REPO/blob/HEAD/LICENSE
Copyright: Copyright (c) 2026 WindInput Contributors
ShortDescription: 轻量、快速、可定制的开源中文输入法
Description: 清风输入法专为五笔和码表输入方案设计，提供五笔 86、五笔拼音混输，同时支持全拼和双拼。
Moniker: windinput
Tags:
- ime
- input-method
- pinyin
- wubi
- 输入法
- 五笔
- 拼音
ReleaseNotesUrl: https://github.com/$REPO/releases/tag/v$VERSION
Documentations:
- DocumentLabel: 使用文档
  DocumentUrl: https://windinput.com/docs
ManifestType: defaultLocale
ManifestVersion: $MANIFEST_VERSION
YAML

cat > "$DIR/$ID.locale.en-US.yaml" <<YAML
$HEADER.locale.$MANIFEST_VERSION.schema.json

PackageIdentifier: $ID
PackageVersion: $VERSION
PackageLocale: en-US
Publisher: 清风输入法 项目
PublisherUrl: https://github.com/huanfeng
PublisherSupportUrl: https://github.com/$REPO/issues
Author: huanfeng
PackageName: WindInput
PackageUrl: https://windinput.com
License: MIT
LicenseUrl: https://github.com/$REPO/blob/HEAD/LICENSE
ShortDescription: A lightweight, fast and customizable open-source Chinese input method
Description: WindInput is designed for Wubi and code-table input schemes, offering Wubi 86 and Wubi-Pinyin mixed input, with Quanpin and Shuangpin support.
Tags:
- ime
- input-method
- pinyin
- wubi
- chinese
ReleaseNotesUrl: https://github.com/$REPO/releases/tag/v$VERSION
ManifestType: locale
ManifestVersion: $MANIFEST_VERSION
YAML

echo "已生成: $DIR"
ls -1 "$DIR"
