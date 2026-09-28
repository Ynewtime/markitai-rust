# HTML article selection

Full HTML extraction selects a coherent content region before rendering it to
Markdown. This policy is separate from the fragment renderer used by EPUB,
Office documents and email: a book's table of contents is useful content.

The selector accumulates visible text and paragraph scores once per DOM node.
Hidden elements and structurally identified page controls do not contribute.
Asides and recognized footnote sections are neutral for selection scoring, so
long external definitions cannot displace a short explicit article. This does
not delete asides inside the selected article or on pages with no article.
It considers `main`, `role=main`, `article`, `itemprop=articleBody` and a bounded
set of established content container names such as `entry-content`,
`post-content` and `blog-post-content`. One content container is selected when
it accounts for at least three fifths of the surrounding region's score.
Substantial introductory or concluding text outside that container keeps the
wider region selected. Multiple sibling articles retain their common main or
body region, which protects portfolios and indexes.

Two explicit structures have more precise boundaries: a GitHub-style README
article with `markdown-body`, `entry-content` and `itemprop=text`, and the
complete `data-testid=issue-viewer-container`. Individual `markdown-body`
comments are not treated as an entire page. Issue discussion content remains;
known metadata sidebars, sticky header duplicates and comment action controls
are excluded.

Page chrome exclusion uses container roles or complete class/ID tokens for
table-of-contents widgets, related/recommended post widgets, subscription
forms, CTA sections, sharing controls and cookie banners. Bare words inside
paragraphs, class substrings and link density do not trigger removal. A generic
Related Posts/Stories heading requires at least two structurally identifiable
linked cards and no independent prose, table, quotation or note content in the
surrounding block. A heading by itself is insufficient.

Explicit footnote/endnote/bibliography containers are protected. The parent
footnote pass resolves referenced definitions against the full document even
when the selected article is narrower. Chrome references do not create notes.
These rules preserve an article about navigation, ordinary comment bodies,
reading lists and unlabelled image galleries. They intentionally leave some
ambiguous material that the reference implementation removes using text and
link-density heuristics, including unmarked trailing link lists, inline
featured comments and gallery-like related tiles without a semantic label.

The design addresses concrete extra content observed in the retained round-five
HTML corpus: `related--inline-related-stories-block`,
`scoring--related-posts-byline`, `general--trailing-cta-newsletter`, and
`general--github.com-issue-56`. The repository fixture
`github-repo--panniantong-agent-reach` provides the README structure. New authored
unit cases reduce these structures to their relevant evidence and include
retention negatives. This document describes policy and fixture provenance;
executed corpus results and any remaining differences belong in validation
records, not an inferred claim of complete extraction parity.

An unresolved exception is the script-stripped LessWrong streaming snapshot:
the article is inside a hidden React segment, with no completion calls or
boundary comments. Both the older Rust output and the round-fourteen timeout
rerun contain only four words. Selection does not unhide this segment based on
size or matching IDs. The [streaming gap record](validation/html-streaming-gap-round14.md)
distinguishes this content loss from the corrected timeout and describes the
separate reference hidden-subtree heuristic and future recovery requirements.
