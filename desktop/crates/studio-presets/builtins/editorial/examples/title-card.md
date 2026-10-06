# Title card (Editorial)

A token-bound recipe; every value below is a semantic token, none is a literal.

| Element | Token bindings |
|---|---|
| Background | fill `color.background` |
| Panel | fill `color.surface`, corner `radius.card`, shadow `shadow.card`, padding `spacing.gutter` |
| Title | `typography.title`, fill `color.title` |
| Subtitle | `typography.heading`, fill `color.caption` |
| Accent rule | stroke `stroke.thick`, color `color.accent` |
| Page margin | `spacing.margin` from every edge |
| Reveal | `motion.easing.enter` over `motion.duration.enter`, siblings staggered by `motion.stagger.tight` |
| Exit | `motion.easing.exit` over `motion.duration.exit` |

Original content, MIT licensed (see `LICENSE.txt`).
