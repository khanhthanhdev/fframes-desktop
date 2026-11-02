# Thí nghiệm: part `{{ }}` cho công thức Typst trong fframes-kit

Ngày 2026-10-25. Máy: Windows Server 2022, chỉ có CPU, MiKTeX 26.5 (cài theo user), Typst 0.15.1,
mitex 0.2.4 (spec tag 0.2.6). Plan: [261025-1020-typst-formula-parts-spike](../261025-1020-typst-formula-parts-spike/plan.md).
Thí nghiệm trước: [experiment-261025-1640-typst-vs-miktex.md](experiment-261025-1640-typst-vs-miktex.md).

## Kết luận: **Go**

Theo luật quyết định viết trước trong phase 7:

- **Phase 3 đạt** (cổng go/no-go): công thức LaTeX đi qua mitex có đúng số glyph mỗi part và
  mỗi label như TeX ở 13/13 trường hợp, marker làm lệch glyph tối đa 0.00000 pt (giới hạn 0.01 pt).
- **Phase 4 đạt**: cả 5 transform của gallery có cùng số group khớp, glyph mờ đi, glyph hiện ra
  với TeX và với Typst, `inspect` không báo lỗi.

Vậy nên viết plan tích hợp Typst vào kit. Hai kết quả phụ quyết định nội dung plan đó:

- **Phase 5:** khớp theo token đưa vào plan tích hợp như một tuỳ chọn, không thay quy tắc mặc định.
- **Phase 6:** transform giữa công thức TeX và công thức Typst không khớp được glyph nào, nên kit
  phải báo lỗi. Với gói `physics`/`siunitx`, kit cần gửi kèm một bộ `\newcommand`; còn `\div`
  thì cần TeX.

## Cách làm

- **Không `eval`.** `eval` gán cùng một span cho mọi node, nên mất vị trí từng glyph. Code mitex
  sinh ra được ghi thẳng vào `main.typ` giữa `$ … $`. File này import `/mitex/flat.typ`, một module
  sinh tự động có một `#let KEY = mitex-scope.at("KEY")` cho mỗi key của scope mitex.
- **Đánh dấu part.** Plan định dùng `CommandSpec` của mitex để thêm lệnh `kitpart`. Cách này không
  chạy được: mitex 0.2.4 chỉ dùng spec riêng khi parse, còn khi chuyển đổi luôn dùng `DEFAULT_SPEC`
  (`converter.rs:1100`), nên lệnh mới báo `unknown command`. Thay vào đó, `{{`, `\part{name}{` và
  `}}` được đổi thành ký tự private-use U+E000, U+E002 và U+E001. mitex giữ nguyên các ký tự này
  và thứ tự của chúng. Sau đó chúng được viết lại thành lời gọi `kitpart(#N, ..)` và
  `kitlabel(#K, ..)`.
- **Gán glyph vào part.** Mỗi glyph và shape của frame Typst có span, span cho ra khoảng byte
  trong `main.typ`. Glyph thuộc part có khoảng chứa nó; label trong cùng thắng. Có hai loại phần
  tử không có span:
  - Gạch phân số, dấu căn: do hàm của mitex vẽ. `kitpart`/`kitlabel` thêm `metadata` đầu và
    cuối. Walker theo dõi `Tag::Start` của chúng để biết phần tử nằm trong part nào.
  - Glyph của `sin`, `cos`, `ln`: span bị tách rời. Chúng được tìm bằng chữ của text item trong
    đoạn source giữa hai glyph có span ở hai bên.
- **Đưa vào kit.** Spike tự viết SVG có nhóm `<g id="part-N">` và `<g id="label-NAME">`, rồi gọi
  `Formula::from_svg`. Từ đó `Formula`, `FormulaView` và `TransformMatchingTex` của kit được dùng
  nguyên vẹn. `fframes-kit/src` không đổi.

Code ở fframes-kit, branch `spike/typst`, commit `ea436b6`, `96c739b`, `b62c7f6`, `33b03ef`, `30f811e`:

- `spikes/typst-probe`: World của Typst, module flat, walker, gán part.
- `spikes/typst-vs-tex`: các phép đo. `cargo run --release -- out --compare-walker --parts
  --latex-parts --token-matching --physics --mixed`.
- `spikes/typst-transforms`: video các transform. `cargo run --release -- matching`, `inspect`,
  `strip <Scene>`.

## Kết quả theo phase

### Phase 1: walker có khoảng source

Walker tự đặt glyph theo đúng phép biến đổi của typst-svg, so với `typst_svg::svg`.

| | Kết quả |
|---|---|
| Công thức typeset được | 22/22 (`\ce` và TikZ bị loại như thí nghiệm trước) |
| Số glyph walker = typst-svg | 22/22 |
| Chênh lệch vị trí glyph và nét lớn nhất | 0.00000 pt (giới hạn 0.01 pt) |

### Phase 2: part trong cú pháp Typst (bước đệm)

| Trường hợp (Typst) | Glyph mỗi part TeX / Typst | Lệch tâm x / y % | Kết quả |
|---|---|---|---|
| `{{a}}^2` | [1] / [1] | 0.0 / 1.5 | đạt |
| `frac({{a}}, b)` | [1] / [1] | 0.0 / 0.0 | đạt |
| `lr(( {{x}} ))` | [1] / [1] | 0.1 / 0.0 | đạt |
| `{{a}} + {{b}} = {{c}}` | [1, 1, 1] / [1, 1, 1] | 0.2 / 0.0 | đạt |
| `{{e}}^({{i}} {{pi}}) + {{1}} = {{0}}` | [1, 1, 1, 1, 1] / [1, 1, 1, 1, 1] | 0.1 / 2.4 | đạt |
| `{{a}}^2 + {{b}}^2 = {{c}}^2` | [1, 1, 1] / [1, 1, 1] | 0.4 / 3.9 | **không đạt** |
| `{{a}}^2 = {{c}}^2 - {{b}}^2` | [1, 1, 1] / [1, 1, 1] | 0.4 / 3.8 | **không đạt** |
| `{{sin}} x` | [3] / [3] | 3.0 / 0.0 | **không đạt** |
| `{{frac(a, b)}} + c` | [3] / [3] | 0.1 / 0.0 | đạt |

6/9 đạt tiêu chí lệch tâm 3%. Cả 9 trường hợp đều gán đúng glyph vào part. Ba trường hợp không đạt
là do bố cục khác nhau, không phải do gán part sai:

- Hai công thức Pythagoras: Typst đặt chỉ số trên thấp hơn TeX. Thí nghiệm trước đã thấy điều này.
- `sin x`: khoảng cách sau toán tử khác nhau.

Phase 2 không phải cổng quyết định.

### Phase 3: part cho LaTeX qua mitex (cổng go/no-go)

**Không eval:** 22/22 công thức có cùng số glyph như khi dùng `eval`. Chênh lệch glyph lớn nhất là
0.00001 pt.

| LaTeX | Glyph mỗi part TeX / Typst | Label TeX / Typst | Marker làm lệch (pt) | Kết quả |
|---|---|---|---|---|
| `{{a}}^2` | [1] / [1] | – | 0.00000 | đạt |
| `\frac{{{a}}}{b}` | [1] / [1] | – | 0.00000 | đạt |
| `\left({{x}}\right)` | [1] / [1] | – | 0.00000 | đạt |
| `{{a}} + {{b}} = {{c}}` | [1, 1, 1] / [1, 1, 1] | – | 0.00000 | đạt |
| `{{e}}^{ {{i}} {{\pi}} } + {{1}} = {{0}}` | [1, 1, 1, 1, 1] / [1, 1, 1, 1, 1] | – | 0.00000 | đạt |
| `{{a}}^2 + {{b}}^2 = {{c}}^2` | [1, 1, 1] / [1, 1, 1] | – | 0.00000 | đạt |
| `{{a}}^2 = {{c}}^2 - {{b}}^2` | [1, 1, 1] / [1, 1, 1] | – | 0.00000 | đạt |
| `{{\sin}} x` | [3] / [3] | – | 0.00000 | đạt |
| `{{\frac{a}{b}}} + c` | [3] / [3] | – | 0.00000 | đạt |
| `\sqrt{{{x}}}` | [1] / [1] | – | 0.00000 | đạt |
| `{{\sqrt{x}}} + 1` | [3] / [3] | – | 0.00000 | đạt |
| `\part{lhs}{ {{a}}^2 + {{b}}^2 } = {{c}}^2` | [1, 1, 1] / [1, 1, 1] | lhs: 5 / lhs: 5 | 0.00000 | đạt |
| `a {{+}} b` | [1] / [1] | – | 0.00000 | đạt |

- 13/13 đạt.
- Phần tử được gán bằng marker và phần tử được gán bằng khoảng byte không mâu thuẫn ở glyph nào.

**Khoảng cách của `{{+}}`** (khoảng a→+ và +→b, pt):

| | `a + b` | `a {{+}} b` |
|---|---|---|
| TeX | 3.077, 3.248 | 0.863, 1.034 |
| Typst | 3.092, 3.252 | 3.092, 3.252 |

Trong TeX, `{{+}}` là một nhóm có ngoặc nhọn nên `+` bị đổi thành ký hiệu thường. Nguyên nhân là
macro `\fframesPart` của kit bọc part trong ngoặc nhọn (`template.rs:51`), chứ người viết không
yêu cầu. Typst giữ khoảng cách của toán tử hai ngôi.

Đặt part vào trong một atom đúng loại sửa được phía TeX mà vẫn giữ part (đo sau, cùng lệnh
`--latex-parts`):

| LaTeX | Khoảng TeX | Part TeX | Khoảng Typst | Part Typst |
|---|---|---|---|---|
| `a + b` | 3.077, 3.248 | – | 3.092, 3.252 | – |
| `a {{+}} b` | 0.863, 1.034 | [1] | 3.092, 3.252 | [1] |
| `a \mathbin{{{+}}} b` | 3.077, 3.248 | [1] | 3.092, 3.252 | [1] |
| `a = b` | 3.630, 3.801 | – | 3.648, 3.808 | – |
| `a {{=}} b` | 0.863, 1.034 | [1] | 3.648, 3.808 | [1] |
| `a \mathrel{{{=}}} b` | 3.630, 3.801 | [1] | 3.648, 3.808 | [1] |

**Quyết định:** giữ khoảng cách toán tử cho `{{+}}` ở cả hai engine. Typst không cần sửa gì. Phía
TeX, kit tự bọc part chỉ gồm một toán tử hai ngôi trong `\mathbin{..}` và part chỉ gồm một quan hệ
trong `\mathrel{..}`. Bỏ ngoặc nhọn khỏi macro thì không được, vì `{{a}}^2` cần ngoặc để số mũ
gắn vào part. Video TeX hiện có sẽ thay đổi: `+` và `=` có part dịch ra khoảng 2.2–2.8 pt mỗi bên
ở 10 pt.

### Phase 4: TransformMatchingTex từ đầu đến cuối

Năm transform của gallery được dựng hai lần, một lần từ công thức TeX và một lần từ công thức Typst,
qua `FormulaView` và `TransformMatchingTex` không sửa đổi.

| Transform | Glyph từ → đến | Group khớp TeX / Typst | Mờ đi TeX / Typst | Hiện ra TeX / Typst |
|---|---|---|---|---|
| Journey1 (`\theta` → `\pi`) | 14 → 14 | 11 / 11 | 3 / 3 | 3 / 3 |
| Journey2 (→ `-1`) | 14 → 6 | 4 / 4 | 10 / 10 | 2 / 2 |
| Journey3 (→ `+ 1 = 0`) | 6 → 7 | 5 / 5 | 1 / 1 | 2 / 2 |
| Identity (→ 5 part) | 7 → 7 | 2 / 2 | 5 / 5 | 5 / 5 |
| Pythagoras → giải ra a² | 8 → 8 | 7 / 7 | 1 / 1 | 1 / 1 |

- Số liệu trùng khớp ở 5/5 transform.
- `inspect` báo "checked 130 frames: no problems found" và thoát với mã 0.
- Các dải hình cho thấy cùng một chuyển động. Mỗi cặp dưới đây gồm TeX trước, Typst sau:
  - Journey1: [TeX](experiment-261025-1803-typst-formula-parts-strip-Journey1Tex.png), [Typst](experiment-261025-1803-typst-formula-parts-strip-Journey1Typst.png)
  - Journey2: [TeX](experiment-261025-1803-typst-formula-parts-strip-Journey2Tex.png), [Typst](experiment-261025-1803-typst-formula-parts-strip-Journey2Typst.png)
  - Journey3: [TeX](experiment-261025-1803-typst-formula-parts-strip-Journey3Tex.png), [Typst](experiment-261025-1803-typst-formula-parts-strip-Journey3Typst.png)
  - Identity: [TeX](experiment-261025-1803-typst-formula-parts-strip-IdentityTex.png), [Typst](experiment-261025-1803-typst-formula-parts-strip-IdentityTypst.png)
  - Pythagoras: [TeX](experiment-261025-1803-typst-formula-parts-strip-PythagorasTex.png), [Typst](experiment-261025-1803-typst-formula-parts-strip-PythagorasTypst.png)

### Phase 5: khớp tự động theo token

Mỗi token lá của LaTeX được đánh dấu thành một part: chữ cái, chữ số, toán tử, hoặc lệnh không có
đối số như `\pi`. Khoá khớp là chữ của token cộng thứ tự lần xuất hiện (`2#1`). Mỗi ô dưới đây ghi:
di chuyển / mờ đi / hiện ra / khớp sai. Khớp sai là khi một glyph bay sang glyph của một token khác.

| Cặp | Theo outline | Part viết tay | Theo token |
|---|---|---|---|
| journey 1 | 11 / 3 / 3 / 0 | 11 / 3 / 3 / 0 | 11 / 3 / 3 / 0 |
| journey 2 | 4 / 10 / 2 / 0 | 4 / 10 / 2 / 0 | 4 / 10 / 2 / 0 |
| journey 3 | 5 / 1 / 2 / 0 | 5 / 1 / 2 / 0 | 5 / 1 / 2 / 0 |
| pythagoras | 7 / 1 / 1 / 0 | 7 / 1 / 1 / 0 | 7 / 1 / 1 / 0 |
| `x^2` → `2x` | 1 / 1 / 1 / 0 | 1 / 1 / 1 / 0 | **2 / 0 / 0 / 0** |
| `\sin x` → `\cos x` | 2 / 2 / 2 / **1** | 2 / 2 / 2 / **1** | 1 / 3 / 3 / 0 |
| `e^{x}` → `e^{x}` | 2 / 0 / 0 / 0 | 2 / 0 / 0 / 0 | 2 / 0 / 0 / 0 |
| `\ln x` → `\frac{1}{x}` | 1 / 2 / 2 / 0 | 1 / 2 / 2 / 0 | 1 / 2 / 2 / 0 |
| `a+b=c` → `a=c-b` | 4 / 1 / 1 / 0 | 4 / 1 / 1 / 0 | 4 / 1 / 1 / 0 |
| `2x+3=7` → `2x=4` | 3 / 3 / 1 / 0 | 3 / 3 / 1 / 0 | 3 / 3 / 1 / 0 |
| `x^2=4` → `x=\pm 2` | 2 / 2 / 2 / 0 | 2 / 2 / 2 / 0 | **3 / 1 / 1 / 0** |

- Khớp theo token tốt ít nhất bằng part viết tay ở 10/11 cặp: di chuyển nhiều bằng hoặc hơn, không
  khớp sai thêm. Nó tốt hơn rõ ở hai cặp: số mũ `2` trở thành hệ số `2`, và `2` sang `\pm 2`.
- Cặp còn lại là `\sin` → `\cos`. Quy tắc outline cho chữ `s` của `sin` bay sang `s` của `cos`,
  nên ghi 1 lần khớp sai. Khớp theo token coi `sin` và `cos` là hai token khác nhau và cho mờ đi;
  không khớp sai nhưng ít glyph di chuyển hơn.
- Kiểm tra chéo: `TransformMatchingTex` của kit trên công thức TeX cho cùng số mờ đi / hiện ra như
  cột "part viết tay" ở cả 11 cặp.

**Quyết định:** plan tích hợp có khớp theo token, nhưng như một tuỳ chọn bật thêm (ví dụ
`TransformMatchingTex::by_tokens()`), không phải quy tắc mặc định. Lý do:

- Lợi ích nhỏ: chỉ 2/11 cặp có lợi.
- Nó cần span, nên chỉ có với công thức Typst. Một transform không nên đổi hành vi khi đổi engine.

### Phase 6: lệnh physics/siunitx và transform trộn engine

20 lệnh hay gặp được chạy qua mitex (không eval) và Typst, và qua TeX với `TexTemplate::physics()`:

| | Trước | Sau khi thêm bí danh ký hiệu |
|---|---|---|
| Typst chạy ngay | 4/20 | 6/20 (`\ket`, `\bra`, `\braket`, `\nabla`, `\hbar`, `\partial`) |
| Thêm `\newcommand` vào đầu công thức | thêm 11 | thêm 13 |
| Alias bằng `CommandSpec` | 0 (lỗi mitex ở trên) | 0 |
| TeX | 20/20 | 20/20 |

**Bí danh ký hiệu.** mitex 0.2.4 vẫn viết các tên ký hiệu mà Typst 0.15 không còn:

| mitex viết | Typst 0.15 có |
|---|---|
| `diff` | `partial` |
| `planck.reduce` | `planck` |
| `angle.l` / `angle.r` | `chevron.l` / `chevron.r` |

Ba dòng `#let` trong module flat sửa được cả ba. Trước khi sửa, `\hbar` và `\partial` lỗi ngay
(`unknown symbol modifier`, `unknown variable: diff`), còn `\pdv`, `\ev` và
`\ket`/`\bra`/`\braket` lỗi khi định nghĩa qua `\newcommand`. Sửa xong không có hồi quy: walker vẫn 22/22,
phase 3 vẫn 13/13, phase 4 vẫn cùng số liệu.

**Lệnh vẫn cần TeX hoặc cần chú ý:**

- `\div` (divergence trong gói `physics`): mitex không cho định nghĩa lại `\div`
  (`error unexpected: "div"`). Lệnh này cần TeX.
- `\braket{\phi}{\psi}`: bản có sẵn của mitex nhận một đối số, ra `braket(phi.alt )psi`, tức 4 glyph
  thay vì 5 như TeX. Kit nên gửi kèm `\newcommand` của nó; khi đó ra 5 glyph.
- `\qty{3}{m}`: TeX ra 4 glyph, Typst với định nghĩa kiểu siunitx ra 2 glyph. Gói `physics` và gói
  `siunitx` đều định nghĩa `\qty`. `TexTemplate::physics()` nạp `physics` trước `siunitx`, và 4
  glyph khớp với nghĩa của `physics`: dấu mở và đóng quanh `3`, rồi `m`. Với `physics`, loại dấu
  theo dấu bao đối số: `\qty(x)` ra ngoặc tròn, `\qty[x]` ngoặc vuông, `\qty{x}` ngoặc nhọn. Nên
  `\qty{3}{m}` rất có thể ra `{ 3 } m`; chưa xem hình để xác nhận. **Quyết định:** cả hai engine dùng
  nghĩa của `physics`, vì đó là thứ TeX đang vẽ, nên video hiện có không đổi. Đơn vị viết bằng
  `\SI`, `\si` hoặc `\unit`. Doc comment của `TexTemplate::physics()` ghi `\qty` thuộc siunitx là
  sai và cần sửa.
- `\ce`, TikZ: vẫn cần TeX, như thí nghiệm trước.

**Trộn engine.**

| Từ (engine) | Đến (engine) | Glyph | Khoá outline trùng | Group khớp / mờ đi / hiện ra |
|---|---|---|---|---|
| `e^{i\pi} = -1` (TeX) | `e^{i\pi} + 1 = 0` (Typst) | 6 → 7 | 0 | 0 / 6 / 7 |
| `e^{i\pi} + 1 = 0` (TeX) | cùng công thức (Typst) | 7 → 7 | 0 | 0 / 7 / 7 |
| `e^{i\pi} = -1` (TeX) | `e^{i\pi} + 1 = 0` (TeX) | 6 → 7 | 5 | 5 / 1 / 2 |
| `e^{i\pi} = -1` (Typst) | `e^{i\pi} + 1 = 0` (Typst) | 6 → 7 | 5 | 5 / 1 / 2 |

Ngay cả cùng một công thức cũng không có glyph nào khớp giữa hai engine, vì font và outline khác
nhau (Computer Modern Type 1 qua dvisvgm, so với New Computer Modern OTF). Transform trộn engine chỉ
làm mọi thứ mờ đi rồi hiện ra. **Kit không được để nó âm thầm mờ đi.** Không sửa core fframes
thì kit chỉ báo được cảnh báo, nên `TransformMatchingTex` báo một `RendererWarning` mà `inspect`
hiển thị (plan [261025-1935-typst-formula-engine](../261025-1935-typst-formula-engine/plan.md)).

## Plan tích hợp cần có gì

1. **API `FormulaEngine`.** Ví dụ `MathTex::engine(FormulaEngine::Typst | Tex)`. Plan phải quyết
   định:
   - engine mặc định khi feature `typst` được bật;
   - `Formula` có ghi lại engine đã tạo ra nó không. Cần có để phát hiện transform trộn engine.
2. **Cargo feature.** Feature `typst` kéo theo `typst` =0.15.1, `mitex` =0.2.4 và font New Computer
   Modern. Feature `latex` hiện có (TeX) vẫn giữ.
3. **Đường typeset**, chuyển từ spike sang:
   - module flat sinh từ key của scope mitex, cùng bí danh ký hiệu và hàm `kitpart`/`kitlabel`;
   - ký tự đánh dấu U+E000/E001/E002;
   - walker theo span, có theo dõi `Tag` và tìm toán tử theo chữ;
   - SVG có nhóm part/label, rồi `Formula::from_svg`.

   Kiểm thử của phase 1 và phase 3 trở thành test của kit.
4. **Lỗi "cần TeX vì …".** Khi mitex báo `unknown command`/`unknown environment`
   (`\ce`, `tikzpicture`, `\div`), lỗi phải nêu lệnh và nói rằng công thức này cần engine TeX.
   Plan quyết định có tự chuyển sang TeX khi máy có TeX hay không.
5. **Prelude physics/siunitx.** Gửi kèm định nghĩa `\newcommand` của 13 lệnh đã đo, với `\qty`
   theo nghĩa của `physics` (xem phase 6), và liệt kê `\div` là cần TeX. Sửa doc comment của
   `TexTemplate::physics()`.
6. **Transform trộn engine báo cảnh báo** qua `inspect` (xem phase 6).
7. **Khớp theo token** là tuỳ chọn, chỉ cho công thức Typst (xem phase 5).
8. **Editor.** Không nhúng Typst vào WASM (thí nghiệm trước: file tải về lớn gấp khoảng 4.7 lần).
   Công thức được typeset sẵn lúc build và nhúng vào binary.
9. **Phía mitex.** Báo lên upstream lỗi `CommandSpec` chỉ dùng khi parse, và các tên ký hiệu cũ.
   Khi nâng mitex, kiểm tra lại xem còn cần ký tự đánh dấu và bí danh không.
10. **Khoảng cách `{{+}}`.** Giữ khoảng cách toán tử ở cả hai engine (xem phase 3). Phía TeX,
    `mark_part` bọc part chỉ gồm một toán tử trong `\mathbin`/`\mathrel`. Plan phải định nghĩa danh
    sách toán tử và quan hệ, cập nhật snapshot `parts-bin`, và kiểm lại dải hình của gallery.

## Lệch so với plan

- **Đánh dấu part.** Plan định dùng alias `CommandSpec` của mitex. Cách này không chạy được vì lỗi
  của mitex 0.2.4 (xem "Cách làm"), nên dùng ký tự private-use và `metadata`. Điều plan ghi trong
  "Known facts" về `CommandSpec` là sai với 0.2.4.
- **Kiểm thử phase 3** là cờ `--latex-parts` trong `typst-vs-tex`, không phải file
  `typst-probe/tests/latex_parts.rs`.
- **Tiêu chí lệch tâm 3% của phase 2** không đạt ở 3/9 trường hợp, do bố cục chứ không do gán part.
  Phase 2 không phải cổng quyết định, và phase 3 không dùng tiêu chí này.
- **Bí danh ký hiệu** được thêm vào module flat trong phase 6, và đã đo trước/sau.
