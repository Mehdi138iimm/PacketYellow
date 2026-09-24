<div align="center">

<img src="assets/banner-1.png" alt="PacketYellow" width="100%"/>

<br/>

# 🟡 PacketYellow

**ابزار دسکتاپ تست شبکه، ساخته‌شده برای واقعیتِ اینترنت ایران**

*Ping · Jitter · Packet Loss · DNS · Speed · Filtering: all measured natively in Rust*

<br/>

![Version](https://img.shields.io/badge/version-0.3.1-FFCC00?style=for-the-badge&labelColor=0A0A0C)
![Tauri](https://img.shields.io/badge/Tauri-2-FFC131?style=for-the-badge&logo=tauri&logoColor=white&labelColor=0A0A0C)
![Rust](https://img.shields.io/badge/Rust-core-DEA584?style=for-the-badge&logo=rust&logoColor=white&labelColor=0A0A0C)
![Platform](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D4?style=for-the-badge&logo=windows&logoColor=white&labelColor=0A0A0C)
![RTL](https://img.shields.io/badge/UI-فارسی%20RTL-239F40?style=for-the-badge&labelColor=0A0A0C)

![Stars](https://img.shields.io/github/stars/Mehdi138iimm/PacketYellow?style=flat-square&color=FFCC00&labelColor=0A0A0C)
![Downloads](https://img.shields.io/github/downloads/Mehdi138iimm/PacketYellow/total?style=flat-square&color=FFCC00&labelColor=0A0A0C)
![Last commit](https://img.shields.io/github/last-commit/Mehdi138iimm/PacketYellow?style=flat-square&color=FFCC00&labelColor=0A0A0C)

<br/>

[**⬇️ دانلود**](https://github.com/Mehdi138iimm/PacketYellow/releases/latest) &nbsp;•&nbsp;
[**✨ امکانات**](#-امکانات) &nbsp;•&nbsp;
[**🚀 اجرا**](#-اجرا-از-سورس) &nbsp;•&nbsp;
[**🧱 ساختار**](#-ساختار-پروژه) &nbsp;•&nbsp;
[**📝 تغییرات**](#-تغییرات)

</div>

<br/>

<div dir="rtl">

## 💡 چرا PacketYellow؟

اسپیدتست‌های معمولی فقط یه عدد بهت میدن. ولی تو ایران سؤال واقعی اینه: **چرا این سایت باز نمیشه؟ فیلتره، تحریمه یا قطعه؟ کدوم DNS الان جواب میده؟ پینگم تا سرور بازی چنده؟**

PacketYellow همه‌ی این‌ها رو یه‌جا جواب میده. همه‌ی اندازه‌گیری‌ها **فقط در Rust** انجام میشه و رابط کاربری فقط عدد واقعی رو نشون میده، بدون حدس و بدون عدد فیک.

<br/>

## ✨ امکانات

<p align="center"><img src="assets/banner-2.png" alt="PacketYellow features" width="100%"/></p>

| | بخش | چی کار می‌کنه |
|:-:|---|---|
| 📈 | **داشبورد زنده** | پینگ لحظه‌ای، جیتر، پکت لاس و نمودار زنده (هر ثانیه یه تیک) |
| 🌐 | **تست سایت‌ها** | ۴ مرحله: DNS، پینگ TCP، HTTPS، RTT واقعی + تشخیص دقیق دلیل: **فیلتر DNS** (`10.10.34.x`)، **فیلتر SNI/TLS**، **تحریم** (403/451)، قطعی، تایم‌اوت |
| 🎮 | **پینگ بازی‌ها** | **۲۶۶ بازی آنلاین** با جستجو و دسته‌بندی؛ سرورهای واقعی Supercell، Battle.net، Riot، Minecraft و لیست زنده‌ی Steam برای CS2 و Dota 2 |
| ⚡ | **تست سرعت** | ۴ اتصال دانلود / ۳ آپلود روی Cloudflare، حذف slow-start، تاخیر زیر بار (Bufferbloat)، حجم مصرفی |
| 🧭 | **بنچمارک DNS** | ۲۰ DNS آماده (Cloudflare، Google، Shecan، ...) + DNS دلخواه؛ میانه، کمینه، درصد موفقیت و تشخیص پاسخ فیلترشده |
| 🛡️ | **سرورهای تونل** | پینگ لینک‌های `vless` / `vmess` / `trojan` / `ss` / `hysteria2` / `tuic` + حالت HTTP از داخل تونل و تشخیص Fake-IP |
| 🔎 | **اطلاعات شبکه** | IPv4/IPv6، ISP، ASN، شناسایی اپراتورهای ایرانی، تشخیص VPN / پروکسی / WARP / CGNAT، Wi‑Fi و هات‌اسپات، MTU و گیت‌وی |
| 🧾 | **لاگ** | خطاها و رویدادهای Rust و UI با فیلتر، جستجو، کپی و ذخیره (حتی panicها) |

<br/>

## 🎨 طراحی

- 🌙 تم تیره با رنگ امضای **زرد**، بدون glow و سایه‌ی اضافه
- 🇮🇷 رابط کاملاً **فارسی و راست‌چین** با فونت Vazirmatn
- 🔠 کمترین سایز متن 12px؛ پینگ لحظه‌ای بزرگ‌ترین عنصر صفحه
- 🪟 تایتل‌بار سفارشی، اسپلش و Onboarding شش‌مرحله‌ای انیمیشنی
- 📴 اجرای **بدون CDN** (فونت و آیکون‌ها با `npm run vendor` محلی میشن)

<br/>

## 🛠️ تکنولوژی

<p align="center">
  <img src="https://skillicons.dev/icons?i=rust,tauri,html,css,js,windows" />
</p>

| لایه | ابزار |
|---|---|
| هسته | Rust · Tokio · reqwest (rustls) · hickory-resolver · if-addrs |
| اپ دسکتاپ | Tauri 2 |
| رابط کاربری | HTML / CSS / JavaScript خالص (بدون React و Vite) · Lucide Icons |

<br/>

## 🚀 اجرا از سورس

### پیش‌نیازها
1. [Rust](https://rustup.rs)
2. [Node.js LTS](https://nodejs.org)
3. ویندوز: **Microsoft C++ Build Tools** (Desktop development with C++) + **WebView2** (روی ویندوز ۱۰/۱۱ معمولاً نصبه)
4. VS Code با افزونه‌های `rust-analyzer` و `Tauri`

### دستورها

</div>

```bash
git clone https://github.com/Mehdi138iimm/PacketYellow.git
cd PacketYellow

npm install
npm run vendor                    # یک بار، با اینترنت: فونت و آیکون‌ها برای اجرای بدون CDN
npm run dev                       # حالت توسعه
npm run build                     # ساخت installer نهایی

npm run tauri icon app-icon.png   # (اختیاری) ساخت همه‌ی آیکون‌ها از روی لوگو
cd src-tauri && cargo test        # تست محاسبات جیتر و پکت لاس
```

<div dir="rtl">

<br/>

## 🧱 ساختار پروژه

</div>

```text
PacketYellow/
├── src-tauri/src/
│   ├── probe.rs      → کلاینت HTTP مشترک، تشخیص IP فیلتر و Fake-IP
│   ├── ping.rs       → پینگ TCP بدون نیاز به ادمین، پینگ موازی چند سرور
│   ├── stats.rs      → min / avg / max، جیتر، پکت لاس
│   ├── monitor.rs    → داشبورد زنده (event: monitor://tick)
│   ├── sites.rs      → تست دسترسی سایت‌ها + دلیل باز نشدن
│   ├── dns.rs        → بنچمارک DNS
│   ├── speed.rs      → تست سرعت دانلود / آپلود با پیشرفت زنده
│   ├── netinfo.rs    → IP عمومی، ISP، ASN، اینترفیس‌ها
│   └── applog.rs     → لاگ داخلی اپ
└── ui/
    ├── index.html    → ساختار رابط (RTL، تم تیره)
    ├── style.css     → استایل
    ├── main.js       → منطق رابط (فقط نمایش داده‌ی Rust)
    └── games.js      → کاتالوگ بازی‌ها و منطقه‌ی سرورها
```

<div dir="rtl">

<br/>

## 📝 تغییرات

<details open>
<summary><b>v0.3.1</b>: پایداری و دقت</summary>

- رفع نامرئی شدن UI وقتی انیمیشن‌های ویندوز خاموشه
- تست سرعت بازنویسی شد: سایز تطبیقی ۲۵→۱۰→۵→۱ مگ، تایم‌اوت، جلوگیری از دو تست همزمان، نمایش خطا به‌جای «0 Mbps»
- بخش **لاگ** (کلید `9`) با ثبت panicهای Rust
- اطلاعات شبکه با ۶ سرویس موازی، شناسایی اپراتورهای ایرانی با ASN، پروکسی سیستم و PAC، Wi‑Fi، هات‌اسپات
- پنجره دیگه maximize و زوم نمیشه
</details>

<details>
<summary><b>v0.3</b>: بزرگ‌ترین آپدیت</summary>

- تست سایت‌ها ۴ مرحله‌ای با تشخیص فیلتر DNS / SNI / تحریم
- ۲۶۶ بازی آنلاین + سرورهای واقعی
- حالت HTTP برای VPN و تشخیص Fake-IP در حالت TUN
- DNS دلخواه + ۲۰ DNS آماده
- تست سرعت چنداتصالی با بافربلوت
- Onboarding، اسپلش و بخش راهنما
</details>

<details>
<summary><b>v0.2</b>: رابط جدید</summary>

- UI تیره‌ی جدید با HTML/CSS/JS خالص
- انتقال همه‌ی اندازه‌گیری‌ها به Rust
- تایتل‌بار سفارشی و ذخیره‌ی تاریخچه
</details>

<br/>

## 🗺️ نقشه‌ی راه

- [ ] پینگ ICMP واقعی با `surge-ping`
- [ ] ذخیره‌ی تاریخچه با SQLite
- [ ] پارس کامل لینک‌های vless / vmess / trojan
- [ ] نمودارهای حرفه‌ای‌تر با uPlot

<br/>

## 🤝 مشارکت

ایده، باگ یا پیشنهاد داری؟ یه [Issue](https://github.com/Mehdi138iimm/PacketYellow/issues) باز کن یا Pull Request بفرست. اگه به کارت اومد، یه ⭐ بده که بیشتر دیده بشه.

</div>

<br/>

<div align="center">

<img src="https://raw.githubusercontent.com/Mehdi138iimm/PacketYellow/main/app-icon.png" width="64"/>

**ساخته‌شده با 💛 توسط [Mehdi](https://github.com/Mehdi138iimm)**

*برای ایران 🦁☀️*

</div>
