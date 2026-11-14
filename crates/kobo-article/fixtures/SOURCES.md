# Test pages

These pages are inputs to `tests/fixtures.rs` only. They are never built
into a binary or shipped to a reader. Each was saved on 8 November 2026 as
the HTML the server sent, without its pictures, scripts run or styles
fetched.

| File | Source | Terms |
| --- | --- | --- |
| `wikipedia-common-kingfisher.html` | "Common kingfisher", English Wikipedia, revision 1379243991: <https://en.wikipedia.org/w/index.php?title=Common_kingfisher&oldid=1379243991>. Text by Wikipedia contributors; the full list of authors is the page history: <https://en.wikipedia.org/w/index.php?title=Common_kingfisher&action=history> | [Creative Commons Attribution-ShareAlike 4.0](https://creativecommons.org/licenses/by-sa/4.0/). Unchanged from the page as served. |
| `nasa-hubble-m30.html` | "Hubble Learns Galactic History From Stars", NASA Science, 5 November 2026: <https://science.nasa.gov/missions/hubble/hubble-learns-galactic-history-from-stars/> | NASA material is not protected by copyright in the United States ([NASA media usage guidelines](https://www.nasa.gov/nasa-brand-center/images-and-media/)). The text credits ESA/Hubble. |
| `nist-microcomb.html` | "New Chip-Based 'Tape Measure' for Light Could Revolutionize Mobile Timekeeping and Enhance National Security", NIST news, 16 October 2026: <https://www.nist.gov/news-events/news/2026/10/new-chip-based-tape-measure-light-could-revolutionize-mobile-timekeeping> | A work of the United States government, not subject to copyright in the United States (17 U.S.C. 105). The photographs the page refers to are credited to their makers and are not included. |
| `gutenberg-yellow-wallpaper.html` | "The Yellow Wallpaper" by Charlotte Perkins Gilman, Project Gutenberg eBook 1952: <https://www.gutenberg.org/ebooks/1952> | Public domain in the United States. The file keeps the Project Gutenberg licence it was distributed with. |
| `synthetic-news.html`, `synthetic-blog.html`, `synthetic-magazine.html` | Written for these tests. The people, places and sites in them are invented. | Part of Cobalt, under its licence. |

The synthetic pages imitate the layouts of common news, blog and magazine
pages, with the furniture those pages carry: cookie and consent banners, menus,
share bars, advertisement slots and labels, newsletter boxes, paywall prompts,
"read more" and "related" links, comment threads, sidebars and sticky footer
advertisements. Their pictures use addresses under `example.org`,
`example.com` and `example.net`, which the tests answer themselves.
