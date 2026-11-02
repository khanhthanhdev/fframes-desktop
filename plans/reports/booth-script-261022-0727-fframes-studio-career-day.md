# Kịch bản booth Tech Career Day: fframes Studio

Kịch bản gồm hai phần, dùng độc lập được:

- **Phần 1: Giới thiệu.** Dành cho mọi khách ghé booth, kể cả người không làm kỹ thuật. Bản đầy đủ khoảng 3 phút, có thêm bản 30 giây.
- **Phần 2: Đi sâu kỹ thuật.** Dành cho sinh viên IT, developer và người phỏng vấn kỹ thuật. Khoảng 8 phút, chia thành các module, dừng ở module nào cũng được.

Chữ trong `[ngoặc vuông]` là chỉ dẫn hành động (demo, chỉ màn hình), không đọc thành tiếng. Thuật ngữ quen thuộc (Rust, GPU, SVG, agent, timeline, render) giữ nguyên tiếng Anh, vì sinh viên IT nghe quen hơn.

---

## Phần 1: Giới thiệu (cho mọi người)

### 1.0. Bản 30 giây (khi khách chỉ đi ngang)

> Chào bạn! Đây là **fframes Studio**, một ứng dụng làm video bằng cách *nói chuyện với AI*.
> Bạn mô tả video mình muốn, AI viết video đó thành code, còn bạn xem trước ngay trên màn hình. Muốn sửa chỗ nào thì bấm vào chỗ đó và nói: "chữ này to hơn", "cảnh này chậm lại". Xong thì xuất ra file MP4.
> Bạn không cần biết lập trình. Bạn có muốn xem thử 2 phút không?

### 1.1. Mở đầu (khoảng 20 giây)

> Bạn đã bao giờ cần làm một video ngắn, như video giới thiệu sản phẩm, video thuyết trình hay clip cho mạng xã hội, mà không biết dùng Premiere hay After Effects chưa?
> Thông thường bạn có hai lựa chọn: tự học phần mềm dựng phim mất vài tuần, hoặc thuê người làm.
> Ở đây mình muốn giới thiệu lựa chọn thứ ba.

### 1.2. fframes Studio là gì (khoảng 30 giây)

> fframes Studio là một ứng dụng desktop. Trong đó bạn làm video theo bốn bước:
>
> 1. **Mô tả**: viết bằng lời video bạn muốn, giống như dặn dò một nhà thiết kế.
> 2. **Xem**: ứng dụng dựng video và cho bạn xem ngay, có timeline giống phần mềm dựng phim.
> 3. **Chỉ và sửa**: bấm vào một vật thể trong video, ví dụ một dòng chữ, rồi nói muốn đổi gì. AI sẽ sửa đúng chỗ đó.
> 4. **Xuất**: xuất ra file video hoàn chỉnh.

### 1.3. Demo (khoảng 90 giây)

`[Mở Studio với một project mẫu đã build sẵn, ví dụ Hello World hoặc Signal Lab]`

> Đây là màn hình chính. Bên phải là video, bên dưới là timeline với các cảnh và âm thanh, bên trái là khung chat với AI.

`[Bấm Play, để video chạy vài giây]`

> Giờ mình muốn đổi dòng tiêu đề này.

`[Bấm vào dòng chữ trên video; video dừng lại và dòng chữ được chọn]`

> Mình chỉ cần bấm vào nó, rồi gõ: "Đổi tiêu đề thành Tech Career Day, màu cam."

`[Gửi yêu cầu. Trong lúc AI làm việc, nói tiếp phần dưới]`

> Trong lúc chờ, có một điểm thú vị: **AI không xem được video**. Nó không có mắt và không có tai. Vì vậy ứng dụng phải biến video thành những thứ AI đọc được: ảnh chụp từng khung hình, danh sách lỗi và số đo âm lượng. AI sửa xong, ứng dụng tự build lại, tự kiểm tra rồi mới hiện kết quả cho bạn.

`[Kết quả hiện ra, bấm Play]`

> Xong rồi. Nếu không thích, mình bấm **Undo** để quay lại bản trước.

`[Kéo, cắt một cảnh trên timeline]`

> Muốn cắt hoặc đổi thứ tự cảnh thì kéo thả trên timeline như phần mềm dựng phim bình thường, không cần nhờ AI.

> **Nếu demo trực tiếp gặp sự cố** (mạng, AI chậm): mở video đã quay sẵn quy trình này và thuyết minh theo đúng các câu trên.

### 1.4. Vì sao nó khác (khoảng 30 giây)

> Có nhiều công cụ AI tạo video, nhưng phần lớn cho ra một đoạn video "hộp đen": muốn sửa một chi tiết thì phải tạo lại từ đầu.
> Ở fframes Studio, video thực chất là **code**, cụ thể là code Rust. Vì vậy:
>
> - Sửa được từng chi tiết nhỏ mà không làm hỏng phần còn lại.
> - Kết quả giống hệt nhau mỗi lần xuất.
> - Bạn dùng AI mình đã có, như Claude Code hay Codex, chứ không bị khóa vào một dịch vụ.
> - Bạn không cần đọc code. Người biết code thì vẫn mở ra sửa tay được.

### 1.5. Kêu gọi hành động (khoảng 15 giây)

> Nếu bạn muốn thử, bản preview cho Windows đang được phát hành miễn phí. Quét mã QR này để vào trang giới thiệu và tải về.
> Nếu bạn là sinh viên IT và muốn biết bên trong nó hoạt động thế nào, mình có thể nói sâu hơn về kỹ thuật!

`[QR dẫn tới trang Studio: https://fframes-studio.pages.dev/ (QR có sẵn ở slide cuối)]`

### 1.6. Câu hỏi thường gặp

**Tôi có cần biết lập trình không?**
> Không. Bạn nói chuyện với AI và bấm trên video. Code nằm phía sau, ai biết code thì có thể mở ra xem.

**Dùng AI nào? Có mất phí không?**
> Studio kết nối với AI lập trình mà bạn đã có, như Claude Code, Codex và một số agent khác. Bạn đăng nhập AI theo tài khoản của mình. Phí (nếu có) là phí của dịch vụ AI đó. Bản thân thư viện fframes là mã nguồn mở, giấy phép MIT.

**Chạy trên máy nào?**
> Hiện tại bản preview dành cho Windows. macOS mới chỉ được build và chạy test tự động, chưa kiểm thử đầy đủ.

**AI sửa hỏng thì sao?**
> Mọi thay đổi của AI phải build thành công và qua kiểm tra rồi mới được áp dụng. Bạn luôn có Undo, có thể xem lại AI đã đổi những gì và chỉ giữ phần mình muốn.

**Làm được loại video nào?**
> Video đồ họa chuyển động: video giới thiệu sản phẩm, video giải thích, title, phụ đề, video cho podcast, biểu đồ chuyển động, thậm chí cảnh 3D đơn giản. Nó không dùng để dựng lại phim quay bằng máy ảnh.

**Ai làm ra nó?**
> Thư viện lõi fframes là dự án mã nguồn mở của cộng đồng (tác giả gốc: dmtrKovalenko trên GitHub). fframes Studio là ứng dụng desktop xây dựng trên nền thư viện đó.

---

## Phần 2: Đi sâu kỹ thuật (cho developer và sinh viên IT)

Gợi ý: hỏi người nghe trước. Ai thích hiệu năng thì nói Module A. Ai thích AI agent thì nói Module C và D. Ai thích kiến trúc hệ thống thì nói Module B và E.

### Module A: Lõi fframes, video là một hàm Rust (khoảng 2 phút)

> Ý tưởng cốt lõi rất đơn giản: **mỗi khung hình là một cây SVG do một hàm Rust trả về**.
> Bạn implement trait `Video`, khai báo FPS, kích thước, độ dài, rồi viết hàm `render_frame(frame)`. Hàm này chạy một lần cho mỗi frame, trên nhiều thread song song.

`[Hiện đoạn code ngắn trong README: Hello World với svgr! và timeline!]`

> - `svgr!` là một procedural macro: bạn viết SVG ngay trong Rust và chèn biểu thức `{...}` vào thuộc tính.
> - `timeline!` mô tả animation bằng keyframe, có easing như EaseIn, Bezier hay Spring vật lý.
> - Âm thanh khai báo bằng `AudioMap`: track nào phát lúc nào, chỉnh gain, fade, tự hạ nhạc nền khi có giọng nói (ducking), có limiter ở master bus.

**Vì sao nhanh:**

> - **Cache lúc biên dịch:** phần SVG nào không có `{biểu thức}` thì macro băm (hash) ngay lúc compile. Renderer nhận ra và tái sử dụng qua các frame, không vẽ lại.
> - **Hai backend vẽ:** tiny-skia chạy CPU ở mọi nơi, còn Skia chạy GPU qua Metal hoặc Vulkan, nhanh hơn khoảng 10 lần ở 1080p.
> - **FFmpeg được link trực tiếp** qua thư viện libav thay vì gọi một tiến trình bên ngoài. Với backend GPU, frame có thể đi thẳng từ GPU vào bộ mã hóa phần cứng mà không cần đọc ngược về CPU.
> - Ví dụ từ dự án gốc: video giới thiệu dài 128 giây của fframes được "vibe code" trong 48 phút và render trong 36 giây.

**Thiết kế cho tác giả không nhìn thấy:**

> AI không xem được video, nên mỗi project có sẵn một CLI biến video thành dữ liệu đọc được:
>
> - `inspect`: quét frame, báo thiếu font, thiếu ảnh, chữ bị cắt khỏi khung, transform NaN, panic, kèm thời điểm và cảnh.
> - `strip`: ghép nhiều frame thành một contact sheet để AI "nhìn" chuyển động.
> - `onion`: chồng nhiều frame lên nhau để thấy quỹ đạo và easing.
> - `audio analyze`: đo loudness (LUFS), true peak, clipping, khoảng lặng.
>
> Đây là điểm mình thấy hay nhất về mặt thiết kế: **API cố tình rõ ràng và dài dòng**. Với con người đó là nhược điểm, nhưng với AI agent thì lại là ưu điểm.

### Module B: Kiến trúc Studio (khoảng 2 phút)

> Studio là ứng dụng native viết bằng Rust, UI dùng **GPUI**, framework UI chạy bằng GPU mà editor Zed sử dụng. Nó là một Cargo workspace riêng với khoảng 10 crate: UI, engine, harness cho agent, SDK, project, preset...

`[Vẽ hoặc chỉ sơ đồ: UI → Engine → Build → Preview worker]`

> Có ba tầng thực thi:
>
> 1. **UI thread** (GPUI): chỉ vẽ, không bao giờ bị chặn.
> 2. **Engine thread**: nhận `Command`, chạy lần lượt từng job, trả về đúng một `EngineEvent` cho mỗi command. Mọi việc nặng như compile, decode, render hay hash đều chạy ở thread hoặc tiến trình khác.
> 3. **Preview worker**: một **tiến trình riêng**, là chính video của người dùng được compile thành file chạy.

**Câu hỏi hay để hỏi ngược khán giả: vì sao worker phải là tiến trình riêng?**

> - **Ranh giới kiểu generic của Rust:** mỗi video là một kiểu `Video` cụ thể khác nhau. App không thể biết trước kiểu đó lúc compile, nên compile video thành chương trình riêng rồi nói chuyện qua protocol.
> - **Cách ly lỗi:** code người dùng hoặc AI viết có thể panic hay crash. Worker chết thì app vẫn sống và giữ bản preview cũ.
> - Lưu ý trung thực: đây là **cách ly crash, không phải sandbox bảo mật**.
>
> Worker nói chuyện với app bằng JSON điều khiển qua stdout, còn pixel và âm thanh PCM đi qua một kênh nhị phân loopback riêng. Mọi message đều mang version, revision và request ID, nên frame cũ không bao giờ đè lên frame mới.

**Managed SDK:**

> Một project Rust cần compiler, linker và thư viện native. Studio tự cài một **SDK được quản lý**: toolchain Rust cố định phiên bản, fframes đã test và FFmpeg build sẵn. Mỗi project được ghim vào một SDK, nên cài SDK mới không làm hỏng project cũ.

### Module C: Kết nối AI agent (khoảng 1,5 phút)

> Studio không tự làm AI. Nó **điều phối** AI lập trình có sẵn qua ba "wire":
>
> - **Claude Code** qua giao thức stream-json.
> - **Codex** qua app-server JSON-RPC.
> - **ACP** (Agent Client Protocol) cho các agent khác.
>
> Cả ba được bọc sau một trait chung `AgentDriver`. UI chỉ hiện những gì agent thực sự hỗ trợ, ví dụ có nhận ảnh không, có "steer" được giữa chừng không.

**Công cụ cho agent:**

> Agent được cấp 9 công cụ **chỉ đọc**, qua MCP hoặc CLI: `project_context`, `selection_context`, `source_lookup`, `timeline`, `render_frame`, `render_strip`, `inspect`, `style_context`, `build_status`.
> Chúng không cho chạy shell, không cho ghi file, không truy cập file tùy ý. Mọi phản hồi đều có giới hạn kích thước.
> Kênh giao tiếp là named pipe trên Windows (Unix socket trên Linux và macOS), chỉ chấp nhận đúng user hiện tại. Mỗi task có một "capability" riêng, bị thu hồi khi task kết thúc.

**Kiểm soát tiến trình AI:**

> Trên Windows, agent được khởi động ở trạng thái tạm dừng, đưa vào một **Job Object** không cho thoát ra, xác nhận xong mới cho chạy. Nhờ vậy Studio biết chắc mọi tiến trình con của agent đều nằm trong tầm kiểm soát, và nút Stop luôn dừng được tất cả.

### Module D: Giao dịch chỉnh sửa, "agent xong" khác "video sẵn sàng" (khoảng 1,5 phút)

> Đây là phần mình tâm đắc nhất. Trong Studio, **"AI báo xong" chưa có nghĩa là "video đã sẵn sàng"**.

`[Chỉ sơ đồ state machine trong architecture.md, mục 5]`

> Quy trình:
>
> 1. AI sửa trên một **bản nháp** (draft working copy) chứ không động vào source thật.
> 2. Khi AI dừng, bản nháp được **chụp hai lần** vào kho lưu trữ theo nội dung (content-addressed). Hai lần khác nhau nghĩa là có thứ vẫn đang ghi, và capture bị hủy. Không bao giờ validate một cây file đang thay đổi.
> 3. **Build**, rồi **inspect**, rồi render các frame đại diện và frame ở biên của vùng được chọn, rồi chuẩn bị audio.
> 4. Nếu lỗi, AI được **tự sửa đúng một lần**. Vẫn lỗi thì dừng và báo người dùng.
> 5. Qua hết thì mới **áp dụng**. Việc ghi file là một giao dịch có journal, có hash trước và sau, dùng rename nguyên tử. Nếu có người khác ghi đè cùng lúc, mọi phiên bản được giữ lại và báo xung đột.
>
> Undo cũng là một giao dịch được validate y như vậy, và chỉ đảo ngược đúng phần của AI, giữ nguyên những gì bạn tự sửa.

### Module E: Các chi tiết nhỏ nhưng khó (chọn 1-2 cái, khoảng 1 phút)

**Bấm chọn trên video:**
> Khi bạn bấm vào video, tọa độ chuột được ánh xạ qua zoom, pan và letterbox về pixel thật, rồi tra metadata **của đúng frame đang hiển thị**. Vùng chọn không bao giờ được "đoán lại" theo tên hay vị trí gần đúng. Nếu preview đã đổi, vùng chọn cũ bị từ chối. Đây là cách AI biết chính xác bạn đang nói về phần tử nào và đoạn code nào sinh ra nó.

**Timeline không cần build lại:**
> Cắt, tách, đổi thứ tự cảnh chỉ ghi vào file `timeline.json`, một tài liệu thời gian nằm ngoài code. Thay đổi thời gian không cần compile lại, nên phản hồi tức thì.

**Audio thời gian thực:**
> Callback âm thanh **không cấp phát bộ nhớ, không lấy lock, không I/O**. Dữ liệu được đẩy qua một ring buffer cố định. Đây là quy tắc kinh điển của lập trình audio real-time, vì chỉ trễ vài mili giây là đã nghe tiếng rè.

**Định dạng file có phiên bản:**
> Mọi file lưu trữ đều có version. File của bản cũ bị từ chối và giữ nguyên từng byte, không có migration ngầm. Chọn sự an toàn thay cho sự tiện lợi.

### Kết phần kỹ thuật (khoảng 15 giây)

> Tóm lại, fframes biến video thành code, và Studio biến việc viết code đó thành một cuộc trò chuyện, với đủ lớp kiểm tra để không tin mù quáng vào AI.
> Code nằm trên GitHub. Nếu bạn quan tâm đến Rust, hệ thống desktop hay AI agent, đây là một codebase rất đáng đọc. Cứ hỏi mình thêm nhé!

---

## Câu hỏi kỹ thuật có thể gặp

**Sao chọn Rust mà không phải Python hay JavaScript?**
> Vì hiệu năng render song song nhiều frame, vì link trực tiếp FFmpeg và Skia, và vì compiler Rust bắt lỗi rất chặt. Với code do AI viết, compiler nghiêm khắc chính là một lớp kiểm tra miễn phí.

**Khác gì Remotion (làm video bằng React)?**
> Cùng ý tưởng "video là code". fframes dùng Rust và SVG, render native trên CPU hoặc GPU, không cần trình duyệt headless. Studio thêm lớp điều phối AI agent và kiểm tra tự động.

**AI có thể chạy lệnh nguy hiểm trên máy không?**
> Công cụ fframes cấp cho AI là chỉ đọc. Bản thân AI agent vẫn có thể xin chạy lệnh, và mỗi yêu cầu sẽ hiện thành thẻ để người dùng duyệt, trừ khi người dùng chọn chế độ cho phép. Studio nói rõ đây không phải sandbox.

**Test kiểu gì cho một app có AI?**
> Agent được thay bằng "fake CLI" và peer có kịch bản để test cả ba wire. Engine được test với compiler giả và worker giả. Các test với SDK thật, audio thật và cửa sổ thật chạy trên CI tự host.

**Có chạy trên máy không có GPU không?**
> Có. Backend CPU chạy ở mọi nơi. Studio hiện preview và export bằng CPU, nên các hiệu ứng shader chỉ có trên Skia GPU sẽ không hiển thị trong Studio.

---

## Ghi chú cho người đứng booth

- **Chuẩn bị trước:** build sẵn project mẫu (`prepare_examples`, xem `docs/desktop/README.md`), đăng nhập sẵn agent, quay sẵn một video demo dự phòng.
- **Máy demo không có GPU:** đừng hứa tốc độ GPU khi demo trực tiếp. Hãy dẫn con số trong tài liệu (Skia GPU nhanh hơn khoảng 10 lần CPU).
- **Trung thực về trạng thái:** đây là bản preview Windows chưa ký số (unsigned), nên Windows có thể hiện cảnh báo khi cài.
- **Đọc khán giả:** người không làm kỹ thuật cần thấy "bấm vào chữ, nói, chữ đổi". Developer cần thấy "agent xong khác video sẵn sàng".

### Bảng thuật ngữ nhanh

| Thuật ngữ | Giải thích ngắn |
| --- | --- |
| Render | Vẽ ra từng khung hình của video |
| Frame | Một khung hình; video 30 fps có 30 frame mỗi giây |
| Timeline | Dải thời gian sắp xếp các cảnh và âm thanh |
| Agent | AI có thể tự đọc, viết code và gọi công cụ |
| SVG | Định dạng đồ họa vector mô tả hình bằng văn bản |
| Backend CPU/GPU | Bộ phận vẽ chạy trên vi xử lý thường hoặc card đồ họa |
| SDK | Bộ công cụ cần có để build project |
| MCP | Giao thức chuẩn để AI gọi công cụ bên ngoài |
| LUFS | Đơn vị đo độ to cảm nhận của âm thanh |
