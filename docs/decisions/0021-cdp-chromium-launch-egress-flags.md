---
status: "accepted"
date: 2026-06-24
decision-makers: thkt (project owner)
---

# CDP Chromium Launch Egress Flags

## Context and Problem Statement

scout の `js-rendering` 経路は chromium を CDP 経由で起動し JS を実行してページを描画する。chromium は単なる HTTP client と違い、ページ JS が `fetch` / WebRTC / DNS prefetch などで scout の意図しない egress を発生させうる。SSRF 防御 (ADR-0001, ADR-0012) は scout 自身の HTTP 経路を connect-time IP guard で守るが、chromium は別プロセスで自前のネットワークスタックを持つため、その egress は起動フラグでしか制御できない。さらに chromium の既定起動には telemetry・更新確認・background networking など、headless 自動化に不要かつ egress を増やす機能が含まれる。

scout は `src/fetch/cdp/launch.rs` の `build_launch_args` で固定フラグ集合を渡す。フラグは (1) background egress を止める hardening 群と (2) chromium の全 TCP egress を scout の loopback SOCKS5 proxy へ強制し connect-time IP を再検証する proxy 群 (issue #201) の二本柱である。このフラグ集合と選定方針が ADR として記録されておらず、コードは存在しない `spec.md` の "Chrome Launch Flags table" を参照したままになっている。

## Decision Drivers

- ページ JS が起こす egress を scout の制御下に置き、SSRF 境界 (ADR-0001/0012) の趣旨を chromium プロセスにも及ぼす
- headless 自動化に不要な background 通信 (telemetry, 更新確認, prefetch, WebRTC) を止め egress を減らす
- DNS rebinding を塞ぐため、target host の名前解決を chromium ではなく proxy 側で行う
- 起動フラグを単一箇所に集約し、各フラグの根拠を追える (存在しない spec への dangling 参照を解消)

## Considered Options

- Option A: hardening フラグ群 + 全 egress を loopback SOCKS5 proxy へ強制するフラグ群を単一箇所で渡す (採用)
- Option B: chromium 既定フラグのまま起動する
- Option C: hardening のみ行い proxy 強制はしない (HTTP 経路の SSRF guard だけに頼る)

## Decision Outcome

Chosen option: Option A。`build_launch_args(proxy_port)` (`src/fetch/cdp/launch.rs`) が固定フラグ集合を組み立てる。hardening 群は `--headless=new` で headless 化し、`--disable-webrtc` / `--disable-background-networking` / `--disable-features=DnsOverHttps` / `--disable-domain-reliability` / `--no-pings` / `--disable-extensions` / `--no-first-run` / `--disable-default-apps` で background egress と副作用を止める。proxy 群は `--proxy-server=socks5://127.0.0.1:{proxy_port}` で全 TCP egress を scout の loopback SOCKS5 proxy へ流し (SOCKS5 なので host 解決を proxy 側が行い DNS rebinding を塞ぐ)、`--proxy-bypass-list=<-loopback>` で chromium の暗黙の loopback / link-local (169.254/16 = IMDS) DIRECT bypass を打ち消してそれらも proxy 経由にし、`--disable-quic` で TCP SOCKS5 が傍受できない QUIC/UDP egress を止める。proxy を通った egress は connect-time に再検証され (ADR-0012)、scout HTTP 経路と同じ SSRF 境界が chromium にも及ぶ。各フラグの根拠は本 ADR とコード近傍コメントに置き、`spec.md` への dangling 参照を本 ADR で置き換える。

Option B は telemetry / prefetch / WebRTC を放置し scout の egress 制御方針に反するため却下。Option C は chromium の egress が scout の SSRF guard を迂回し、ページ JS が内部サービスへ到達しうるため却下。

### Consequences

- Good, because 全 chromium TCP egress が loopback SOCKS5 proxy 経由になり、connect-time IP 再検証 (ADR-0012) で内部到達を防ぐ
- Good, because SOCKS5 + `--proxy-bypass-list=<-loopback>` が host 解決を proxy 側に寄せ、loopback / link-local (IMDS) bypass と DNS rebinding を塞ぐ
- Good, because hardening 群が telemetry / 更新確認 / WebRTC 等の background egress を止め、ページ取得に不要な通信を減らす
- Good, because フラグが `build_launch_args` の単一箇所に集約され、各フラグの根拠を本 ADR から追える
- Bad, because chromium のバージョン更新でフラグ名 / 既定が変わると、無効化したはずの機能や bypass が復活しうる (バージョン追従が要る)
- Bad, because `--disable-quic` で QUIC を切るため QUIC 専用配信のサイトは HTTP/2 等へ fallback できないと取得できない
- Bad, because egress 制御は TCP SOCKS5 で傍受可能な経路に限られ、傍受不能な egress を増やす将来のフラグ追加は別途 proxy-bypass との整合を評価する必要がある

### Confirmation

`src/fetch/cdp/launch/cdp_launch_tests.rs` がフラグ集合を pin する。`[T-F043]` は launch 引数に hardening フラグ (`--disable-webrtc` / `--disable-background-networking` / `--disable-features=DnsOverHttps` / `--disable-domain-reliability` ほか) が含まれることを assert する。`[T-201-8]` は SOCKS5 `--proxy-server=socks5://127.0.0.1:{port}` と `--proxy-bypass-list=<-loopback>` / `--disable-quic` が含まれることを assert する。chromium / CDP ライブラリを更新する際はこれらと実起動 smoke でフラグが依然有効かを再検証し、egress 可能な新フラグを足す際は proxy-bypass との整合を評価する。

## Pros and Cons of the Options

### Option A: hardening 群 + loopback SOCKS5 proxy 強制群 (採用)

background egress を止め、全 TCP egress を proxy へ強制し connect-time に再検証する。

- Good, because SSRF 境界の趣旨を chromium プロセスへ広げ、egress も抑える
- Good, because DNS rebinding と IMDS bypass を塞ぐ
- Bad, because chromium バージョン追従が要る

### Option B: chromium 既定フラグ

何も足さず起動する。

- Good, because 実装が単純でバージョン追従不要
- Bad, because telemetry / WebRTC / prefetch を放置し、egress が scout の制御外になる

### Option C: hardening のみ

background egress は止めるが proxy 強制をしない。

- Good, because proxy 運用の複雑さが無い
- Bad, because ページ JS の egress が SSRF guard を迂回し内部サービスへ到達しうる

## More Information

### フラグ (一次ソース `src/fetch/cdp/launch.rs` の `build_launch_args`)

| 群        | フラグ                                                               | 目的                                                               |
| --------- | -------------------------------------------------------------------- | ------------------------------------------------------------------ |
| hardening | `--headless=new`                                                     | headless 実行                                                      |
| hardening | `--disable-webrtc`                                                   | WebRTC 経由の egress / IP 漏洩を止める                             |
| hardening | `--disable-background-networking`                                    | background 通信全般を止める                                        |
| hardening | `--disable-features=DnsOverHttps`                                    | DoH による proxy 迂回名前解決を止める                              |
| hardening | `--disable-domain-reliability`                                       | Google への信頼性レポート送信を止める                              |
| hardening | `--no-pings`                                                         | hyperlink auditing ping を止める                                   |
| hardening | `--disable-extensions` / `--no-first-run` / `--disable-default-apps` | 拡張・初回フロー・既定アプリの副作用を止める                       |
| proxy     | `--proxy-server=socks5://127.0.0.1:{proxy_port}`                     | 全 TCP egress を loopback SOCKS5 proxy へ (host 解決を proxy 側に) |
| proxy     | `--proxy-bypass-list=<-loopback>`                                    | loopback / link-local (IMDS) の暗黙 DIRECT bypass を打ち消す       |
| proxy     | `--disable-quic`                                                     | TCP SOCKS5 が傍受できない QUIC/UDP egress を止める                 |

### SSRF 境界との関係

ADR-0001 / ADR-0012 は scout HTTP 経路の SSRF を connect-time IP guard で守る。chromium は別プロセスのため、proxy フラグで全 egress を scout の loopback SOCKS5 proxy へ寄せ、proxy が connect-time に同じ guard を適用する (`src/fetch/cdp/launch.rs` の `check_browser_request` の subrequest scheme 判定も併用)。本 ADR はこの launch フラグ層を扱う。

### 参照切れの解消

`src/fetch/cdp/launch.rs` の `build_launch_args` の doc comment は、存在しない `spec.md` の "Chrome Launch Flags table" を参照していた。その表はリポジトリに無く、launch フラグの決定根拠は本 ADR が単一の真実源として置き換える。source コメントの参照差し替えは横流しタスクで追跡する。

### 参照

- `src/fetch/cdp/launch.rs` の `build_launch_args`、`check_browser_request` (subrequest scheme 判定)
- `src/fetch/cdp/proxy.rs` (loopback SOCKS5 proxy 実装)
- ADR-0001 / ADR-0012 (scout HTTP 経路の SSRF 境界。chromium は proxy 経由で同じ guard を継承)
- `docs/audit/2026-06-24-020601-adr-gaps.md` (本 ADR の根拠 audit、候補 keep #9 / #18)
