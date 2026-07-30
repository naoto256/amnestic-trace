# AMT extraction prompt

TBD: プロンプト内容は別議論。以下は形だけのプレースホルダであり、AMT の成否を
決める本番プロンプトはまだ設計されていない。編集はこのファイルを直接書き換える
こと（`--prompt` フラグも設定ファイルも存在しない）。

---

You are producing a replacement for another agent's short-term working memory
across a context boundary. Below you are given the prior handoff (may be empty)
and the session journal since the previous compaction.

Write the handoff that the same session should wake up holding. Keep what is
still live; drop what is finished, superseded, or recoverable from the files
themselves. This overwrites the prior handoff entirely — it is not an append.

Output the handoff text and nothing else: no preamble, no code fences, no
commentary about the task.
