---
status: "accepted"
date: 2026-05-30
decision-makers: thkt (project owner)
---

# Connect-time IP Guard for SSRF DNS Rebinding, with CDP Path Asymmetry

## Context and Problem Statement

`ssrf_check` (src/fetch/ssrf.rs:108-138) は DNS 解決で private IP を弾いてから `ValidatedUrl` を返すが、`ValidatedUrl(url::Url)` はドメイン名を保持し IP を捨てる。reqwest は connect 時に OS resolver で独自に再解決するため、TTL=0 の DNS rebind で `public→169.254.169.254` (cloud metadata) に到達しうる。critic-evidence で verified。

これは ADR-0001 が定めた SSRF contract「全 fetch 経路で private IP 帯への到達を遮断」に対する実証された穴であり、ADR-0001 の Reassessment Trigger「SSRF contract 違反 incident」に該当する。fetch.rs:170-172 の "Local CLI only. TOCTOU gap ... acceptable" コメントは、scout の主 consumer が AI エージェント (クラウドで検索結果やユーザー指示由来の信頼できない URL を fetch する) である以上、threat model として成立しない。

## Decision Drivers

* OUTCOME Constraint「全 fetch 経路で SSRF 防御を必須とする (private IP 帯への到達を遮断)」を額面通り守る
* AI エージェント consumer はクラウド上で信頼できない URL を踏むため、人間が対話的に使う CLI より rebind リスクが高い
* reqwest 0.13.3 は custom DNS resolver 注入を default API (`ClientBuilder::dns_resolver`) で提供する
* CDP/chromium 経路は chromium が独自に DNS 解決し、起動オプション依存の別アーキになる

## Considered Options

* 方式 Y': reqwest `Resolve` を実装する `SsrfResolver` を `fetch_http` に注入し connect 時に検証、`ssrf_check` pre-flight は維持
* 方式 X: 検証済み host→IP をマップ共有する `PinnedResolver`
* 方式 Z: リクエストごとに `resolve_to_addrs` で client を構築
* 方式 B: コード変更せず OUTCOME Constraint に "Local CLI の TOCTOU 許容" を明記

## Decision Outcome

Chosen option: 方式 Y'。`SsrfResolver` (reqwest `Resolve` 実装) を `fetch_http` に `dns_resolver` 注入し、connect 時に解決→`is_private_ip` 検証→private なら reject する。`ssrf_check` pre-flight は維持し `ValidatedUrl` 型契約と `InternalHost` (sysexits 65) UX を保つ。

方式 Y' は connector が実際に dial するアドレスで private 判定するため rebind を原理的に閉じる。方式 X は「検証済みのその IP に connect する」性質に security value がない (constraint が問うのは private か否かだけで、それは dial するアドレスで検証すれば足りる) 一方、マップの lifecycle・並行競合・host-key 不一致による正当 host の誤ブロックを足すため却下。方式 Z は ADR-0001 が記録した単一 `fetch_http` invariant を破壊し、redirect 先 host が client 構築時に未知のため却下。方式 B は OUTCOME Constraint を緩める方向で、クラウド実行の AI エージェントに metadata 露出を残すため却下。

CDP 経路の非対称: chromium 経路 (js-rendering feature、default 無効の opt-in) は connect 時 guard が効かない。chromium が独自に解決・connect するため、`check_browser_request` (src/fetch/cdp/launch.rs) の resolve 時 `ssrf_check` のみが効き、rebind 穴は残る。今回スコープ外とし別 issue で追跡する。OUTCOME Constraint にこの非対称を carve-out として明記する。

### Consequences

* Good, because reqwest 経路の DNS rebind を遮断する (connector が検証済みアドレスのみ dial)
* Good, because 状態を持たない (方式 X のマップを排除し、並行競合・誤ブロックの懸念が消える)
* Good, because `ssrf_check` pre-flight を維持するため通常の private host ブロックは sysexits 65 + warn のまま不変
* Bad, because CDP/chromium 経路には rebind 穴が残る (opt-in だが別 issue #201 で塞ぐまで非対称)
* Bad, because pre-flight と connect で DNS を 2 回解決する (二重防御の意図的コスト、DNS キャッシュで緩和)

### Confirmation

rebind 回帰テスト (Spec T-003): pre-flight 用に public を、connect 用に private を別 resolver で注入し、`"blocked connect to private IP"` warn が出ることを assert する。guard が壊れていても connect 失敗で `is_err()` は真になるが warn は出ないため、ログ assert が壊れた guard を検出する (非トートロジー)。reqwest 更新時は `Resolve` が新規接続毎に consult され connector が返却アドレスのみ dial する挙動 (本 ADR の前提) を再検証する。

## Pros and Cons of the Options

### 方式 Y' (採用)

reqwest `Resolve` 実装を `fetch_http` に注入 + `ssrf_check` pre-flight 維持。

* Good, because connect 時に実 dial アドレスで private 判定し rebind を閉じる
* Good, because 状態を持たず ValidatedUrl 契約と InternalHost UX を保つ
* Bad, because CDP 経路は別アーキで本方式が届かない

### 方式 X (PinnedResolver マップ共有)

`ssrf_check` 検証済み host→IP を `Arc<RwLock<HashMap>>` に記録し reqwest が読む。

* Good, because DNS 解決が 1 回で済む
* Bad, because 「検証済みのその IP に connect」は security value ゼロ (private 判定だけが要件)
* Bad, because マップ lifecycle・並行競合・host-key 不一致 (case/trailing-dot/punycode) による正当 host の誤ブロックを足す

### 方式 Z (per-request client)

`ValidatedUrl` に IP を埋め `resolve_to_addrs` で client を都度構築。

* Good, because connect アドレスを明示的に固定できる
* Bad, because ADR-0001 の単一 `fetch_http` invariant を破壊する
* Bad, because redirect 先 host が client 構築時に未知で per-hop 再構築が必要

### 方式 B (OUTCOME に TOCTOU 許容を明記)

コード変更なしで Constraint に例外を書く。

* Good, because 実装コストゼロ
* Bad, because OUTCOME Constraint を緩め、クラウド AI エージェントに metadata 露出を残す

## More Information

### Threat Model

攻撃者が DNS を制御し、scout を実行する AI エージェントがクラウド (AWS/GCP) 上で動く場合、TTL=0 の rebind で fetch 対象を `169.254.169.254` に向け、metadata service から IAM 認証情報を exfiltrate しうる。AI エージェントは検索結果やユーザー指示由来の信頼できない URL を fetch するため、この経路は現実的。

### reqwest 0.13.3 一次ソース確認

* `reqwest::dns::Resolve` trait (resolve.rs:21-34): `fn resolve(&self, name: Name) -> Resolving`、`Name::as_str()` で host 取得
* `HttpConnector<DynResolver>` (connect.rs:34): (a) 新規接続毎に resolver を consult、(b) IP リテラルは resolver を bypass、(c) connector は返却アドレスのみ dial し OS 再解決しない (HappyEyeballs も検証セット内に束縛)
* `ClientBuilder::dns_resolver<R: IntoResolve>` (client.rs:2287): `Arc::new(resolver)` で注入可

### 参照

* issue #193 (本 ADR の対象)、#184 (親 issue、sub ①② は解決済み)
* ADR-0001 (SSRF contract と dual client invariant)、ADR-0009 (`DnsResolver` trait)
* SOW / Spec: `.claude/workspace/planning/2026-05-30-ssrf-connect-ip-guard/`
* CDP carve-out: OUTCOME Constraint に明記、chromium 経路の rebind は別 issue #201 で追跡
