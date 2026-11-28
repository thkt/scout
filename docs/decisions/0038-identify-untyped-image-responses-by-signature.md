---
status: "proposed"
date: 2026-11-28
---

# Content-Type が画像を名乗らない応答を先頭バイトで画像と判定する

## Context and Problem Statement

[Issue #549](https://github.com/thkt/scout/issues/549) は、`scout fetch <URL> --output <path>` で Content-Type が画像を名乗らない画像も保存できるようにすることを求めている。基準版は `05044a4`。[DR-0037](0037-save-image-responses-to-a-caller-named-file.md) の `open_image`（`src/fetch/download.rs`）は、`ImageFormat::from_content_type` が認める 4 種の Content-Type だけを画像とし、それ以外を本文を読まずに `NotAnImage`（終了 65）で拒否する。そのため、Content-Type ヘッダが無い応答、`application/octet-stream` の応答、`image/jpg` と表記した応答は、本文が PNG や JPEG でも保存できない。

DR-0037 の More Information は、シグネチャで判定する案を Issue #547 の受理範囲の外として採っていない。本記録はその案を採る。DR-0037 の本文は変更しない。

## Decision Drivers

- 画像を変換せずに届ける経路を、配信側の Content-Type の付け方に左右させない。オブジェクトストレージやリリース成果物の配信は `application/octet-stream` を返すことがある。
- 画像以外の型を名乗る応答を画像として保存しない。`text/html` を名乗る応答の先頭が偶然シグネチャに合っても、配信側の宣言を優先する。
- DR-0037 の `--js` の条件を保つ。`--js` は媒体の判定の直後かつ本文の読み込みの前に拒否する。
- 本文の読み込み量は、ヘッダで判定できる経路と同じ程度に抑える。

## Considered Options

- 本文の先頭 12 バイトだけを読み、4 形式のシグネチャで判定する
- 本文を上限の 10,000,000 バイトまで読んでから判定する
- Content-Type の種類にかかわらず、すべての応答をシグネチャで判定する
- 変更しない

## Decision Outcome

Chosen option: 「本文の先頭 12 バイトだけを読み、4 形式のシグネチャで判定する」。4 形式のシグネチャはどれも先頭 12 バイトに収まる。最長は WebP の `RIFF`、4 バイトの大きさ、`WEBP` の 12 バイトである。判定に本文全体は要らないので、`--js` の拒否を本文の読み込みより前に置ける。

| 項目 | 決定 |
| --- | --- |
| シグネチャで判定する応答 | Content-Type ヘッダが無い応答と、essence が `application/octet-stream` の応答。大文字小文字とパラメータは問わない |
| ASCII でない Content-Type ヘッダ | ヘッダが無い応答とは扱わず、従来どおり `NotAnImage` で拒否する。ヘッダはあるので、配信側は何かを宣言している |
| `image/jpg` | `ImageFormat::from_content_type` が `image/jpeg` の別表記として受け付ける。保存結果の `content_type` は `ImageFormat::mime` の `image/jpeg` になる。`--output` の無い fetch の `UnsupportedContentType` の hint も、`--output` の案内になる |
| 先頭バイトの読み込み | `open_image` が `SIGNATURE_PREFIX_BYTES`（12）以上たまるか EOF まで chunk を読み、`ImageFormat::from_signature` で照合する。4 形式のシグネチャは先頭バイトが互いに異なるので、合う形式は高々 1 つである |
| どの形式にも合わない場合 | `NotAnImage` で拒否し、ファイルを作らない。payload は essence（ヘッダが無いときは空文字）のままなので、`--output` を外す案内はヘッダが無い応答にだけ出る |
| 残りの本文 | `ImageResponse::read` が `read_rest_capped`（`src/body_limit.rs`）で読む。上限は読み済みの先頭バイトを含めた全体に掛かる。Content-Length の事前確認には、先頭バイトを読む前に取った値を使う |
| `--js` | `fetch_image`（`src/fetch.rs`）の判定の位置は変えない。`open_image` が先頭バイトで画像と決めた後、`ImageResponse::read` の前に `ImageWithJs` で拒否する。本文が上限を超える画像でも、上限超過より先にこの拒否を返す |

Content-Type が 4 種の画像を名乗る応答の経路は変わらない。`open_image` は本文を読まず、`ImageResponse::read` が本文全体を読んでから `ImageFormat::matches_signature` で照合する。`--output` の無い fetch の挙動も、`image/jpg` の hint を除いて変わらない。

### Consequences

- Good, because Content-Type を付けない配信や `application/octet-stream` で配る画像も、変換せずに保存できる。
- Good, because シグネチャで判定する応答でも、`--js` の拒否が本文全体の読み込みより前に来る。
- Bad, because `open_image` がヘッダだけで判定を終えない経路ができ、判定のためにネットワークから最大 1 chunk を読む。
- Bad, because ヘッダが無い応答は、`--output` の有無で扱いが分かれる。`--output` があれば画像として判定し、無ければ従来どおり HTML として変換する。

### Confirmation

`tests/fetch_image.rs` の T-C072 から T-C077 で、ヘッダ無しと `application/octet-stream` の 4 形式の保存、12 バイトに満たない本文、`image/jpg`、画像でない本文の拒否と hint、ASCII でない Content-Type、`--js` の拒否と上限を超える本文を確かめる。既存の T-C061 から T-C071 は変更しない。`src/body_limit/tests.rs` の T-BL003 と T-BL004 で、読み済みの先頭バイトが上限に数えられることを確かめる。

## More Information

本文を上限まで読んでから判定する案は、`--js` と本文の大きさの両方に影響するので採らない。`--js` の拒否が本文の読み込みの後になり、DR-0037 の条件を満たさない。加えて、ヘッダで画像と分かる経路は本文を読まずに拒否できるのに、この経路だけ最大 10,000,000 バイトを読むことになる。

すべての応答をシグネチャで判定する案は採らない。`text/html` などを名乗る応答を画像として保存すると、配信側の宣言と保存結果の `content_type` が食い違う。

実 Web でこの応答に当たった事例は、2026-11-28 時点で確認していない。必要性の根拠は Issue #549 の「根拠: 決定」行にある。

### Reassessment Triggers

- 4 形式以外の画像形式を受け付けることになり、そのシグネチャが 12 バイトを超える場合
- `application/octet-stream` 以外の汎用の型（`binary/octet-stream` など）で画像を配る事例が出た場合
- `--output` の無い fetch でも、ヘッダが無い応答を先頭バイトで判定する要求が来た場合
