#!/usr/bin/env bash
#
# 三个脚本共用的函数。别单独执行，用 `. "$HERE/lib.sh"` 引入。
#
# 存在的理由：验签这件事 `sign.sh`（签完自验）、`deploy.sh`（发之前验）、
# `verify.sh`（验线上）都要做。写三遍就会有三个版本，改一处漏两处。

# ⚠️ **必须先过这道**：macOS 自带的是 **LibreSSL**，它做不了 Ed25519。
#
# 实测（`PATH=/usr/bin:/bin`）：LibreSSL 3.3.6 连私钥都载不进来 ——
# `unsupported private key algorithm: TYPE=Ed25519`，且 `pkeyutl` 没有 `-rawin`。
#
# 不预检的后果比「报个错」糟得多：`sign.sh` 会吐一屏 LibreSSL 用法然后失败，
# 接着 `verify.sh` 报「线上配置验签失败」并建议你重跑 `sign.sh` ——
# **一份完全正确的配置被误诊成签名出错**，而真正的原因是 openssl 的品种不对。
#
# 这台机器能跑只因为 Homebrew 的 OpenSSL 3.x 在 PATH 里抢在 `/usr/bin` 前面。
# 另外两台（newmac / sgmac）只有 LibreSSL ⇒ 在那儿跑必踩。
require_openssl_with_ed25519() {
  if ! openssl pkeyutl -verify -help 2>&1 | grep -q -- '-rawin'; then
    echo "✘ 当前 openssl 不支持 Ed25519 裸签名（缺 -rawin）：$(openssl version)" >&2
    echo "  macOS 自带的是 LibreSSL，做不了这件事。装并优先用 Homebrew 的 OpenSSL：" >&2
    echo "    brew install openssl@3" >&2
    echo "    export PATH=\"\$(brew --prefix openssl@3)/bin:\$PATH\"" >&2
    return 1
  fi
}

# 从 remote_config.rs 里取一个 `const NAME: &str = "..."` 的值。
#
# **从代码里 grep 而不是手抄** —— 手抄就多一个能写错的地方，
# 而写错的症状是「验签永远失败」，与「服务器挂了」一模一样。
rc_const() {
  local name="$1" rs="$2"
  local value
  value=$(grep -E "^const ${name}" "$rs" | sed -E 's/.*"(.*)".*/\1/')
  if [ -z "$value" ]; then
    echo "✘ 在 ${rs} 里找不到 const ${name}" >&2
    return 1
  fi
  printf '%s' "$value"
}

# 按**客户端契约**校验 config.json，而不只是「是不是合法 JSON」。
#
# ## 为什么语法检查不够
#
# 客户端那边 `Sponsor.site_origin` 与 `display_name` **没有 `#[serde(default)]`**
# （只有 `tagline` 有）⇒ 任一缺失会让 `serde_json::from_slice` 失败，
# 而那是**整份配置**失败：连 `aff_codes` 一起丢，静默回落到编译期内置表。
# 一份缺 `display_name` 的 JSON 语法完全合法，`json.tool` 一路放过。
#
# `aff_codes` 的 key 同理：`https://www.Example.com:443` 语法没问题，
# 但客户端按归一后的 host 查表（小写 / 去 www. / 去端口，见 `aff.rs::lookup_host`），
# 这个 key **永远匹配不到** —— 不报错，只是那个站的返利静默丢失。
validate_config_json() {
  local config="$1"
  python3 - "$config" <<'PY'
import json, sys, re

path = sys.argv[1]
with open(path, encoding='utf-8') as f:
    try:
        cfg = json.load(f)
    except json.JSONDecodeError as e:
        sys.exit(f"✘ 不是合法 JSON：{e}")

errors = []

if not isinstance(cfg.get('sponsors', []), list):
    errors.append("sponsors 必须是数组")
for i, s in enumerate(cfg.get('sponsors', [])):
    # 这两个字段客户端**没有** serde default，缺了会让整份配置报废。
    for field in ('site_origin', 'display_name'):
        if not s.get(field):
            errors.append(f"sponsors[{i}] 缺 {field}（客户端会丢弃整份配置）")
    origin = s.get('site_origin', '')
    if origin and not origin.startswith('https://'):
        errors.append(f"sponsors[{i}].site_origin 必须是 https:// 开头：{origin}")

codes = cfg.get('aff_codes', {})
if not isinstance(codes, dict):
    errors.append("aff_codes 必须是对象")
else:
    for host in codes:
        # 与 aff.rs::lookup_host 同一套归一：小写、无 www.、无端口、无 scheme、无路径。
        if host != host.lower():
            errors.append(f"aff_codes key 必须全小写：{host}")
        if host.startswith('www.'):
            errors.append(f"aff_codes key 必须去掉 www. 前缀：{host}")
        if '://' in host or '/' in host:
            errors.append(f"aff_codes key 是纯 host，不带 scheme 或路径：{host}")
        if ':' in host:
            errors.append(f"aff_codes key 不能带端口：{host}")
        if not re.match(r'^[a-z0-9.-]+$', host):
            errors.append(f"aff_codes key 不像一个 host：{host}")

if errors:
    sys.exit("✘ config.json 不符合客户端契约：\n" + "\n".join(f"  - {e}" for e in errors))
PY
}

# hex 公钥 → DER SubjectPublicKeyInfo 文件（openssl 只吃 DER/PEM，不吃裸 hex）。
#
# Ed25519 的 SPKI 前缀是固定的 12 字节，后面直接跟 32 字节裸公钥。
pubkey_hex_to_der() {
  local hex="$1" out="$2"
  {
    printf '302a300506032b6570032100'
    printf '%s' "$hex"
  } | xxd -r -p > "$out"
}

# 用 hex 公钥验一份文件的签名。返回 0 = 通过。
#
# 顺带把「签名长度必须恰好 64 字节」也判了 —— 客户端判等而不是判上限
# （`ED25519_SIGNATURE_LEN` 是定值），不符合就说明那不是我们写的签名文件。
verify_signature() {
  local config="$1" sig="$2" pubkey_hex="$3"

  local size
  size=$(wc -c < "$sig" | tr -d ' ')
  if [ "$size" != "64" ]; then
    echo "✘ 签名长度不对：期望 64 字节，实际 ${size}" >&2
    return 1
  fi

  local der
  der=$(mktemp)
  pubkey_hex_to_der "$pubkey_hex" "$der"
  local rc=0
  openssl pkeyutl -verify -pubin -inkey "$der" -keyform DER \
    -rawin -in "$config" -sigfile "$sig" > /dev/null 2>&1 || rc=1
  rm -f "$der"
  return $rc
}
