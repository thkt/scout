---
status: "accepted"
date: 2026-06-24
decision-makers: thkt (project owner)
---

# Search Default Output: One URL Per Line

## Context and Problem Statement

scout の `search` コマンドはクエリ結果を AI エージェントへ返す。結果の主用途は「次に fetch する URL の取得」であり、既定の markdown 出力がタイトル・snippet を含む冗長ブロックだと、エージェントは続く `scout fetch` のために URL を抽出する後処理を強いられ token も余計に食う。一方でタイトル・スコア等のメタを必要とするエージェントもいるため、メタを捨てるのは惜しい。

scout は `search` の markdown 出力を 1 行 1 URL のプレーン text にし、同じ呼び出しの JSON envelope (ADR-0010) の `data` フィールドに `sources` 全体を構造化して常に同梱する。markdown フィールドと data フィールドの二面性により、URL リストだけ欲しい consumer と構造化メタが欲しい consumer の双方をフラグ無しで満たす。この「markdown = URL リスト、data = 構造化 sources を常に両載せ」という出力契約が ADR として記録されていない。

## Decision Drivers

- search 結果の主用途は「次に fetch する URL」であり、URL リストが最も直接的に次アクションへ繋がる
- 1 行 1 URL は grep / head / xargs / `while read` と自然に合成できる
- タイトル・snippet を必要とするエージェントもいるため、メタは捨てず構造化で同梱する
- これは OUTCOME.md に紐づく公開出力互換の約束で、安定して維持する

## Considered Options

- Option A: markdown = 1 行 1 URL、data = 構造化 sources を同じ envelope に常に両載せ (採用)
- Option B: 既定でタイトル + snippet + URL のリッチブロックを markdown に出す
- Option C: markdown 自体を JSON 配列にする

## Decision Outcome

Chosen option: Option A。`Scout::search` (src/tools/query.rs:20-44) は Brave の結果 `sources` を `sources.iter().map(|s| s.url.as_str()).join("\n")` で 1 行 1 URL の markdown にし、`CommandOutput::ok(markdown, data)` で返す。`data` には `{ "query", "sources" }` として全 source (タイトル等を含む構造化) を常に同梱する。markdown を読む consumer は後処理ゼロで `scout fetch` へ URL を渡せ、`--json` で envelope を読む consumer は `data.sources` から構造化メタを得る。出力モードを分けるフラグは持たず、二面性は envelope の markdown / data フィールドが担う。結果 0 件のとき markdown は空文字列で、`run` の出力層 (src/lib.rs:44-54) が phantom な空行を出さないよう真の空出力を保つ。

Option B は最頻ユースケース (URL を次へ渡す) に後処理を強い token を食うため markdown 既定にしない。Option C は人間直読と行指向ツールに冗長で相性が悪いため markdown 既定にしない。どちらの構造化情報も `data` に既に載るため、markdown 側をリッチにする必要が無い。

### Consequences

- Good, because markdown 出力が `scout fetch` の入力・shell パイプにそのまま渡せ、後処理ゼロで次アクションへ繋がる
- Good, because 行指向で grep / head / xargs と合成でき、エージェントの token も最小
- Good, because 構造化メタは同 envelope の `data.sources` に常在し、必要な consumer は `--json` で取れる
- Good, because 0 件時に真の空出力を保ち、行指向の下流が phantom 空行を読まない
- Bad, because markdown 単独では title / snippet が見えず、URL の良し悪しを URL だけで判断するか `data` を読む必要がある
- Bad, because markdown の URL は Brave から得た生 URL をそのまま join しており、fetch/research 経路 (ADR-0014) のような scheme 中和を経ない (search source は検索エンジン由来で攻撃面が異なるため現状未中和、将来 fetch されると ADR-0001 の SSRF 防御が効く)

### Confirmation

`src/tools/query_tests.rs` が出力契約を pin する。`search_returns_plain_url_list` は stdout が 1 行 1 URL で markdown 装飾を含まないこと、`search_zero_results_returns_empty` は 0 件時に空出力になること、`search_json_schema_omits_answer` / `search_does_not_traverse_engine_path` は `search` が research の engine 経路を通らず envelope schema が期待どおりであることを assert する。markdown フォーマットを変える際はこれらが回帰を検出する。

## Pros and Cons of the Options

### Option A: markdown = URL リスト + data = 構造化 sources 両載せ (採用)

markdown を URL リスト、data を構造化 sources にし常に両方返す。

- Good, because 最頻ユースケース (次の fetch) に後処理ゼロで繋がり、メタも失わない
- Good, because フラグ無しで二種の consumer を満たす
- Bad, because markdown 単独では結果の文脈 (title/snippet) が見えない

### Option B: リッチブロック markdown

title + snippet + URL を markdown に出す。

- Good, because markdown だけで結果の文脈が分かる
- Bad, because URL 抽出の後処理を強い token を食う (data に既に載るため冗長)

### Option C: markdown を JSON 配列に

markdown 自体を構造化配列にする。

- Good, because 機械可読
- Bad, because 人間直読と行指向ツールに冗長で、data と二重化する

## More Information

### 出力契約 (一次ソース src/tools/query.rs:20-44)

| フィールド | 内容                                                 | 用途                           |
| ---------- | ---------------------------------------------------- | ------------------------------ |
| markdown   | `sources` の url を `\n` 連結 (1 行 1 URL、装飾なし) | 次の fetch 入力、shell パイプ  |
| data       | `{ "query", "sources" }` (sources は構造化全体)      | メタが要る consumer (`--json`) |

コメント (query.rs:31-32): "Default output: one URL per line, no markdown decoration. OUTCOME.md: AI agents receive raw source URLs without intermediate summary."

### 参照

- `src/tools/query.rs:20-44` (`search` ハンドラ)
- `src/tools/query_tests.rs` (`search_returns_plain_url_list` ほか)
- `src/lib.rs:44-54` (空出力の保持)
- ADR-0010 (JSON envelope。markdown / data の二面性契約)
- `docs/audit/2026-06-24-020601-adr-gaps.md` (本 ADR の根拠 audit、候補 keep #8 / #15)
