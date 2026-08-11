---
status: "accepted"
date: 2026-06-24
decision-makers: thkt (project owner)
---

# GitHub Token Resolution Precedence and Leak Containment

## Context and Problem Statement

scout の GitHub 経路は認証に bearer token を使う。供給源は複数あり (環境変数 `GITHUB_TOKEN`/`GH_TOKEN`、`gh auth token` subprocess の stdout)、どれを優先するかが曖昧だと CI とローカルで別のトークンが選ばれ再現性が壊れる。さらに `gh auth token` は外部プロセスで、認証失敗時に stderr へトークンや認証情報を echo する実装があるため、その stderr を素通しすると secret が scout の log へ漏れる。

scout は `src/token_source.rs` で `GITHUB_TOKEN` → `GH_TOKEN` → `gh auth token` の固定優先順で解決し、subprocess の stderr を読まず stdout のみ採用し、`TOKEN_RESOLVE_TIMEOUT = 5s` で `gh` の hang を打ち切り、得たトークンを `Redacted` (ADR-0015) に載せる。この優先順・leak 抑制・timeout が ADR として記録されていない。

## Decision Drivers

- CI とローカルで同じ解決順を保証し再現性を確保する (`gh` 慣習に合わせ `GITHUB_TOKEN` を最優先)
- `gh auth token` subprocess が stderr へ吐く secret を scout の出力へ漏らさない
- `gh` が認証プロンプトや network で hang した場合に scout 全体を止めない
- 解決したトークンは Redacted (ADR-0015) に載せ Debug/log 漏洩を防ぐ

## Considered Options

- Option A: 固定優先順 (env 2 種 → `gh` subprocess) + stderr 不採用 + 上限付き timeout + Redacted 格納 (採用)
- Option B: `gh auth token` を最優先にし env を fallback にする
- Option C: env のみ対応し `gh` 連携を持たない

## Decision Outcome

Chosen option: Option A。`resolve_from_env_or_gh` が `["GITHUB_TOKEN", "GH_TOKEN"]` を順に読み、最初に `Redacted::new` を通る (非空・非 whitespace) 値で確定する。両 env が未設定なら `gh auth token` を `kill_on_drop(true)` で起動し、`TOKEN_RESOLVE_TIMEOUT = 5s` を超えたら未認証へ落ちる。subprocess は stdout のみ `Redacted::new` に渡し、stderr は読まない。非ゼロ終了時の warn も exit code のみ報告し stderr を withhold する (SEC コメント)。いずれも取れなければ unauthenticated (60 req/h) として継続する。空文字列・whitespace のみの env は `Redacted::new` が弾くため未設定として次候補へ送られる。

Option B は `gh` 慣習 (`GITHUB_TOKEN` 最優先) に反し CI の明示 env が握り潰される上、毎回 subprocess を起動して遅いため却下。Option C は `gh auth login` 済みユーザーに毎回 env 設定を強い UX を落とすため却下。

### Consequences

- Good, because env 最優先で CI が明示トークンを確実に使え、ローカルは `gh auth` を継承できる
- Good, because subprocess stderr を読まず warn でも withhold するため、`gh` がそこへ吐く secret が漏れない
- Good, because 5s timeout と `kill_on_drop` が `gh` の認証プロンプト hang から scout を守る
- Good, because Redacted 格納で解決後のトークンが Debug/panic message に出ない (ADR-0015)
- Bad, because stderr を捨てるため `gh` の有用な診断 (期限切れ・scope 不足) も失われ、未認証としか分からない
- Bad, because 空/whitespace env を未設定扱いするため、誤って空にした env が黙って次 source へ流れる
- Bad, because `gh` バイナリ不在・非 PATH 環境では subprocess が即失敗し未認証になる (env 併用で回避可能)

### Confirmation

`src/token_source.rs` のテストが解決順を pin する。`[T-TOK001]` は env reader が `GITHUB_TOKEN` を返すと subprocess へ落ちずその値で短絡することを assert する。`[T-TOK002]` は `GITHUB_TOKEN` が whitespace のみのとき未設定扱いで次候補 `GH_TOKEN` へ落ちることを assert する。`gh` 経路も注入で覆う。`resolve_from_env_or_gh` は env reader と並べて subprocess 起動関数を受け取り、production の `GhCliSource` だけが実 `gh` を起動する `spawn_gh` を渡す。`[T-TOK003]` が stdout の末尾改行を trim してトークンにすること、`[T-TOK004]` が非ゼロ終了で stderr をログへ出さないこと (SEC 判断の pin)、`[T-TOK005]` が whitespace のみの stdout を未取得扱いにすること、`[T-TOK006]` が timeout 超過で未認証へ落ちることを assert する。`TokenSource` trait 自体は `StaticTokenSource` で上位からも subprocess 無しにテストできる。

この注入を入れるまで `[T-TOK001]`/`[T-TOK002]` は env 経路しか通らず、`gh` の出力契約が変わっても検出できなかった。そもそもどの分岐を走るかが、テストを回すマシンに `gh` が入っているかに依存していた。

## Pros and Cons of the Options

### Option A: 固定優先順 + stderr 不採用 + timeout + Redacted (採用)

env 2 種を先に見て `gh` を最後に試し、subprocess の stderr を読まず上限付きで待つ。

- Good, because `gh` 慣習に沿い CI 再現性と leak 抑制を両立する
- Good, because hang から守られ secret が漏れない
- Bad, because `gh` の診断情報を失う

### Option B: `gh auth token` 最優先

subprocess を先に試し env を fallback にする。

- Good, because `gh auth login` 済みなら設定不要
- Bad, because CI の明示 env を握り潰し再現性を壊す
- Bad, because 毎回 subprocess 起動で遅い

### Option C: env のみ

`gh` 連携なし。

- Good, because 実装が単純で subprocess 依存が無い
- Bad, because `gh auth` 済みユーザーに env 設定を強いる

## More Information

### 解決順 (一次ソース src/token_source.rs:26-93)

| 優先 | source                               | 採用条件                                      |
| ---- | ------------------------------------ | --------------------------------------------- |
| 1    | env `GITHUB_TOKEN`                   | `Redacted::new` を通る (非空・非 whitespace)  |
| 2    | env `GH_TOKEN`                       | 同上                                          |
| 3    | `gh auth token` subprocess の stdout | 終了 0 且つ非空、`TOKEN_RESOLVE_TIMEOUT` 以内 |
| —    | いずれも無し                         | unauthenticated (60 req/h) として継続         |

`const TOKEN_RESOLVE_TIMEOUT: Duration = Duration::from_secs(5)` (src/token_source.rs:15)。subprocess は `kill_on_drop(true)`、stdout のみ `Redacted::new` へ。非ゼロ終了時の warn は exit code のみ報告し stderr を withhold する (SEC, :80-90)。

### 参照

- `src/token_source.rs:15` (`TOKEN_RESOLVE_TIMEOUT`)、`:26-93` (`GhCliSource`/`resolve_from_env_or_gh`)
- `src/github.rs` (`Auth resolution order` コメント、未認証時の rate limit ヒント)
- ADR-0015 (Redacted。解決トークンの carrier)
- ADR-0019 (env-var fail-fast/timeout 規約と整合)
- `docs/audit/2026-06-24-020601-adr-gaps.md` (本 ADR の根拠 audit)
