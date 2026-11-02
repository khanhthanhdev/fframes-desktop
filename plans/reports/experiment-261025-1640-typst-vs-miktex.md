# Thí nghiệm: Typst và MiKTeX chạy song song cho công thức của fframes-kit

Ngày 2026-10-25. Máy: Windows Server 2022, chỉ có CPU, MiKTeX 26.5 (cài theo user).

## Kết luận

**Có, kit nên có Typst, nhưng chạy cùng MiKTeX chứ không thay hẳn.**

- Typst nên dùng cho công thức toán (`MathTex`). Lúc đầu chỉ cho công thức không có part `{{ }}`. Typst chỉ thành engine mặc định khi việc gán part đã chạy được, vì `TransformMatchingTex` cần part (các công thức `identity`, `pythagoras`, `solved` của gallery dùng chúng) và thí nghiệm này chưa kiểm tra part.
- MiKTeX vẫn giữ cho phương trình hóa học (`ChemEq`, `\ce`), TikZ và những tài liệu cần preamble TeX riêng.
- Không nên nhúng Typst vào WASM của editor. Nó chạy được, nhưng làm file tải về lớn thêm khoảng 4.7 lần. Editor nên dùng kết quả đã typeset sẵn và nhúng vào binary.

"Chạy 2 cùng lúc" được hiểu là: đưa cùng một đầu vào cho cả hai engine rồi so sánh từng công thức.

## Đã đo những gì

- Đầu vào là 18 công thức thật của gallery (`gallery/src/scenes/latex.rs`), giữ nguyên văn: 16 công thức toán, 1 `ChemEq`, 1 TikZ. Thêm 6 công thức hay gặp trong video giải thích: tích phân với `\sqrt`, `pmatrix`, `\vec`, `cases` có `\text`, `\lim`, `\mathbb`.
- **Bên TeX:** `MathTex`, `ChemEq` hoặc `Tex` cùng `TexTemplate::tikz()` của kit, đúng như bây giờ. Đo hai lần: chưa có cache (thư mục cache mới) và đã có cache.
- **Bên Typst:** LaTeX được mitex 0.2.4 chuyển sang Typst. Typst 0.15.1 biên dịch trong tiến trình, xuất SVG bằng `typst_svg::svg`, rồi đưa vào `Formula::from_svg` của kit. Hai engine vì thế đi qua cùng bộ tách glyph và cùng kiểu `Formula`. Code Typst do mitex sinh ra được `eval` trong scope Typst của mitex (`packages/mitex/specs`, tag 0.2.6), giống cách package mitex làm.
- **Hình so sánh:** cùng một renderer (resvg) vẽ glyph của `Formula` cho cả hai bên.
- **WASM:** một crate thử chỉ gồm Typst và hai font, build cho `wasm32-unknown-unknown` (opt-level z, LTO, strip, không chạy wasm-opt) rồi chạy trong Node. Mốc so sánh là editor bridge của gallery, build với cùng profile.

Code thí nghiệm nằm trong fframes-kit, branch `spike/typst`, chưa commit:
- `spikes/typst-probe`: World của Typst, có font và scope mitex nhúng sẵn.
- `spikes/typst-vs-tex`: chương trình so sánh. Chạy bằng `cargo run --release -- out`.

## Kết quả

### Typeset được bao nhiêu

| | MiKTeX | Typst + mitex |
|---|---|---|
| 16 công thức toán của gallery | 16/16 | 16/16, số glyph trùng khớp 100% |
| 6 công thức thêm | 6/6 | 6/6, số glyph trùng khớp 100% |
| `ChemEq` (`\ce`, mhchem) | ✅ | ❌ `unknown command: \ce` |
| TikZ | ✅ | ❌ `unknown environment: \tikzpicture` |
| **Tổng** | **24/24** | **22/24** |

Hình so sánh (TeX bên trái, Typst bên phải): [experiment-261025-1640-typst-vs-miktex-sheet.png](experiment-261025-1640-typst-vs-miktex-sheet.png)

### Hình dạng

- Bề rộng giống TeX trong khoảng 1–2%. Chiều cao thấp hơn 5–9% ở công thức có số mũ: Typst đặt chỉ số trên thấp hơn một chút.
- Hai khác biệt nhìn thấy được:
  - `pmatrix`: Typst hẹp hơn 16% vì khoảng cách cột nhỏ hơn.
  - `cases`: Typst hẹp hơn 7% và thấp hơn 20%.
- Font New Computer Modern rất giống Computer Modern. Bằng mắt khó phân biệt hai cột.

### Tốc độ (ms)

| | Mỗi công thức |
|---|---|
| MiKTeX, chưa có cache | 913 – 1774 (thường khoảng 1050) |
| MiKTeX, đã có cache (đọc file) | 0.4 – 10 |
| Typst, công thức mới | 0.6 – 2.4 |
| Typst, công thức lặp lại (bộ nhớ đệm comemo) | 0.1 – 0.8 |
| Typst, khởi động lần đầu (thư viện, font, scope mitex) | 26 một lần |

Typst nhanh hơn TeX chưa cache khoảng 500 lần, và ngang hoặc nhanh hơn TeX đã cache.

### WASM

| | Thô | gzip | brotli |
|---|---|---|---|
| Editor bridge của gallery hiện tại | 6.26 MB | 2.29 MB | 1.67 MB |
| Typst + 2 font (NewCMMath, NewCM10) | 22.95 MB | 8.42 MB | 6.35 MB |
| Typst không có font | 21.02 MB | 7.18 MB | 5.40 MB |

- Biên dịch sạch cho wasm32, không gặp lỗi `getrandom` (`cargo tree -i getrandom` không tìm thấy crate này) và không import gì từ host.
- Chạy trong Node: công thức đầu tiên 50 ms, các công thức sau 1–5 ms. Công thức lỗi trả về lỗi chứ không panic.
- Hai font chỉ chiếm khoảng 2 MB. Phần lớn kích thước là compiler: thư viện chuẩn của Typst kéo theo hayagriva (trích dẫn), syntect (tô màu code), wasmi (plugin), ICU và các bộ giải mã ảnh. Typst 0.15.1 không có feature flag để bỏ những phần này.

## Đánh giá theo từng mục tiêu

1. **Render không cần cài MiKTeX:** đạt cho toán (22/22), không đạt cho hóa và TikZ. Gallery chỉ có 2 trên 18 công thức cần TeX. Riêng chemistry kit thì mọi `ChemEq` và `Reaction::typeset` đều cần TeX.
2. **Tốc độ:** khoảng 1 ms so với khoảng 1 s. Lợi thật sự là ở lần đầu, khi đổi công thức, và khi Studio cho sửa công thức trực tiếp (Studio là app native nên dùng được Typst native).
3. **Công thức hiện trong web editor:** chạy được nhưng tốn thêm 8.4 MB gzip, tức editor tải về nặng gấp khoảng 4.7 lần. Không đáng làm mặc định. Có một phát hiện liên quan (suy ra từ code, chưa chạy editor để xác nhận): `TexEngine::svg` trên wasm32 đọc cache bằng `std::fs`, mà wasm32-unknown-unknown không hỗ trợ, nên editor có lẽ không hiển thị công thức nào kể cả khi đã có cache. Điều này mâu thuẫn với thông báo lỗi wasm32 của kit ("render natively once to fill the cache"). Cách rẻ để sửa là nhúng kết quả đã typeset vào binary. Cách này đúng cho cả hai engine và chỉ tốn vài KB.

## Đề xuất

1. Thêm Typst vào fframes-kit sau một feature riêng (`typst`), làm engine native cho `MathTex`: trước tiên cho công thức không có part, và thành mặc định khi part đã chạy được. Viết LaTeX vẫn được nhờ mitex. Không cần cache và không cần MiKTeX.
2. Giữ `TexEngine` cho `ChemEq`, TikZ và `TexTemplate` tùy chỉnh. Nếu muốn bỏ TeX khỏi chemistry kit, có thể chuyển `2H2 + O2 -> 2H2O` sang Typst math ngay trong kit (`Reaction` của kit đã tách được các chất), nhưng đó là việc riêng.
3. Cho editor: nhúng kết quả typeset vào binary thay vì chạy Typst trong WASM.
4. Ghim chính xác phiên bản `typst` và `mitex` (API của Typst 0.x đổi theo từng bản) và vendor scope mitex cùng tag.

## Rủi ro chưa kiểm tra

- **Part `{{ }}` chưa được kiểm tra.** Thí nghiệm bỏ hết các part. `TransformMatchingTex` và `color_map` cần biết glyph nào thuộc part nào. Hướng đi là dùng `span` của glyph trong Typst, nhưng code do mitex sinh ra được `eval` từ một chuỗi, nên span trỏ vào chuỗi đó chứ không vào LaTeX gốc. Đây là phần khó nhất nếu triển khai.
- Hình mới khác hình cũ khoảng 5–9% chiều cao và ở khoảng cách cột của ma trận, nên snapshot của gallery sẽ đổi.
- mitex 0.2.4 trên crates.io cũ hơn package Typst của nó (0.2.6/0.2.7). Scope tag 0.2.6 chạy được với cả 22 công thức, nhưng chưa thử hết các lệnh.
- Chưa chạy wasm-opt. Kích thước thật có thể nhỏ hơn 10–20%, nhưng kết luận vẫn vậy.

## Câu hỏi còn mở

- Chemistry kit quan trọng đến đâu? Nếu `ChemEq` được dùng nhiều thì MiKTeX vẫn là phụ thuộc lâu dài, trừ khi viết bộ chuyển hóa học sang Typst trong kit.
- Có muốn Studio cho người dùng sửa công thức trực tiếp không? Nếu có, Typst native đáng làm ngay.
- Bước tiếp theo nên là lập kế hoạch tích hợp (feature `typst`, cách xử lý part, nhúng kết quả cho editor) hay thử trước riêng phần part bằng span?
