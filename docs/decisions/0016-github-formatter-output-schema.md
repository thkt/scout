---
status: "accepted"
date: 2026-06-24
decision-makers: thkt (project owner)
---

# GitHub Formatter Output Schema and README Byte Cap

## Context and Problem Statement

scout の GitHub repo 探索 (`repo-overview` 等) は結果を AI エージェント向けの markdown テキストとして出力する。エージェントはこの出力を parse して repo メタデータ・README・issue/PR/release を抽出するため、出力構造が回ごとに変わると parse が壊れ、特に README が無制限だと context token 予算を予測不能に食う。

`src/github/format.rs` は決定的な section 順序の markdown schema を持ち、README を `MAX_README_BYTES = 24_000` で UTF-8 安全に切り詰める。この出力契約 (section 構成、byte cap、切り詰め方) が ADR として記録されていない。これは JSON envelope (ADR-0010) や error 分類 (ADR-0003) とは別の、テキスト整形そのものの契約である。

## Decision Drivers

- エージェントは安定した出力構造に依存して section を抽出する
- 大規模 repo の長大な README は context token を予測不能に消費する
- multibyte (CJK, emoji) 本文を byte 境界で切ると文字が壊れる

## Considered Options

- Option A: 固定 section 順序の markdown schema + README を 24,000 byte で UTF-8/改行安全に切り詰め (採用)
- Option B: README を切らず全文を出す
- Option C: 行数ベースで切り詰める (旧 baseline)

## Decision Outcome

Chosen option: Option A。`format_overview` がタイトル・メタデータ表・README・Recent Issues・Recent Pull Requests・Recent Releases を決定的な順序で出す。空の section は省く。README は `MAX_README_BYTES = 24_000` を超えた場合のみ `floor_char_boundary` で UTF-8 文字境界へ丸め、その範囲内の最後の改行へ snap し、`shift_headings` で見出し level を下げた後に `(truncated: showing {end} / {total} bytes)` マーカーを付ける。byte cap は ASCII/CJK を跨いで正規化できるため行数 cap より予測可能である。

Option B は token 予算を無制限にし、agent context overrun のリスクを残すため却下。Option C は byte サイズが本文の文字種で変動し ASCII/CJK 間で予測不能なため却下。

### Consequences

- Good, because 決定的で上限のある出力が agent token overrun を防ぐ
- Good, because `floor_char_boundary` で multibyte 文字を分割せず、CJK/emoji が壊れない
- Good, because 改行 snap と見出し shift で切り詰め後も markdown 構造が保たれる
- Good, because `(truncated: ...)` マーカーが切り詰めをエージェントに知らせ、必要なら full README を別取得できる
- Bad, because 24KB を超える README の後半 (API ドキュメント等) は失われる
- Bad, because cap は固定でエージェントから設定できない (at-a-glance outcome に固定)
- Bad, because 切り詰めは schema 上の silent truncation で、エージェントがマーカーを確認しないと欠落に気づかない

### Confirmation

`src/github/format/overview_tests.rs` の `[T-GF006..T-GF036]` が schema を網羅する。`[T-GF008]` は cap 超過で切り詰めが発火し shown bytes が cap 以下になること、`[T-GF020/T-GF021]` は cap 以下・cap ちょうどで切り詰めマーカーが付かないこと、`[T-GF022]` は最後の改行へ snap すること、`[T-GF023]` は改行が無くても panic しないこと、`[T-GF024/T-GF025]` は CJK が文字境界で切れること、`[T-GF036]` はマーカーが見出し shift の対象外であることを assert する。`MAX_README_BYTES` を変える際はこれらが回帰を検出する。

## Pros and Cons of the Options

### Option A: 固定 schema + 24,000 byte UTF-8/改行安全切り詰め (採用)

section 順序を固定し README のみ byte cap で安全に切る。

- Good, because 決定的・上限付きでエージェント parse と token 予算に最適
- Good, because UTF-8/改行/見出し構造を保ったまま切る
- Bad, because cap が固定で後半本文を失う

### Option B: README 全文

切り詰めない。

- Good, because 情報欠落が無い
- Bad, because token 予算が無制限になり agent context overrun のリスク

### Option C: 行数ベース切り詰め

N 行で切る。

- Good, because 行単位は人間に直感的
- Bad, because byte サイズが文字種で変動し予測不能

## More Information

### 出力 section 順序 (一次ソース src/github/format.rs:106-267)

| 順  | section                               | 関数                                        | 省略条件       |
| --- | ------------------------------------- | ------------------------------------------- | -------------- |
| 1   | タイトル (escape 済み) + メタデータ表 | `format_overview` / `format_metadata_table` | —              |
| 2   | `## README`                           | `format_readme_section`                     | README が None |
| 3   | `## Recent Issues`                    | `format_issues_section`                     | 空             |
| 4   | `## Recent Pull Requests`             | `format_pulls_section`                      | 空             |
| 5   | `## Recent Releases`                  | `format_releases_section`                   | 空             |

メタデータ表: Language, Stars, Forks, Open Issues, License, Default Branch, Topics, URL。

### `repo-tree` の出力 (`format_tree`)

`format_overview` とは中和の方法が異なる。ヘッダ (`owner/repo (ref: X)` と `files: N`) は escape せず素で出す。`owner`/`repo` は `parse_repo` が `[A-Za-z0-9._-]` のみを通し、`ref_` は `validate_ref` が `[` を含む文字群を弾くため、いずれも link 記法を組み立てられない。

パス一覧は fence で囲む。パスは GitHub API 由来で検証を通っておらず、`docs/[draft](old).md` のような名前が markdown 上で link として解釈される。ここで `escape_md_inline` を使わないのは、この一覧を読んだエージェントがパスをそのまま `repo-read` の引数に渡すため。escape すると `docs/\[draft\]\(old\).md` が渡り、表示の問題が 404 に変わる。fence はバイトを変えずに block 全体を中和する。fence の長さは `fence_delimiter` がパス中の最長バックティック連から決める。

`src/github/format/tree_tests.rs` の `[T-GF042]` がパスのバイト一致と fence の存在を、`[T-GF043]` がバックティックを含むパスで fence が伸びることを assert する。

### 切り詰めロジック (src/github/format.rs:153-174)

```
const MAX_README_BYTES: usize = 24_000;
let boundary = content.floor_char_boundary(MAX_README_BYTES); // UTF-8 安全
let end = content[..boundary].rfind('\n').map(|p| p + 1).unwrap_or(boundary); // 改行 snap
out.push_str(&shift_headings(&content[..end], 2)); // 見出し level を 2 下げる
// その後にマーカー付与 (マーカー自体は shift 対象外)
```

`truncate_with_note` を再利用しないのは、切り詰めとマーカー付与の間に `shift_headings` を挟む必要があるため。

### README の setext 見出し

`shift_headings` は ATX (`# Title`) に加えて setext (`Title` の次行に `=====` / `-----`) も対象にし、後者は ATX へ書き換えてから level を下げる。h3 以降に setext の表現が無いため、記法の変換を伴う。underline 行は消える。

対象を広げた根拠は実測である。star 上位 100 リポジトリの README のうち 5 件が setext を使い、うち `torvalds/linux` と `996icu/996.ICU` は README ほぼ全体が setext で構成される。ATX 限定のままだと、これらの README は見出しが `## README` より浅い level のまま残り、section 順序の契約が README 内部から壊れる。

`-` は thematic break およびリスト記号と字面が重なるため、CommonMark §4.3 の規則 (underline の直上が段落行なら setext、空行なら thematic break) で判別する。`src/markdown.rs` の `[T-MD021..T-MD025]` が変換・thematic break の非変換・段落でない行の非変換・fence 内の非変換・h6 clamp を assert する。

### 参照

- `src/github/format.rs:6` (`MAX_README_BYTES = 24_000`)、`:106-267` (整形一式)
- `src/github/format/overview_tests.rs` (T-GF006..036)
- ADR-0010 (JSON envelope 契約。本 ADR はテキスト整形契約で別レイヤ)
- `src/github/types.rs:72` の source コメントは `#67/ADR-0010` を参照する (旧 `ADR-0065` 不在参照は差し替え済み)。JSON 出力契約は ADR-0010 が担う
- `docs/audit/2026-06-24-020601-adr-gaps.md` (本 ADR の根拠 audit、候補 #8)
