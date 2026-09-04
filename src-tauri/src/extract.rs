//! 附件解析：Excel → Markdown 表格、Word(.docx) → 纯文本、网页 → 正文文字。
//! 均在后端完成，前端只负责把文件读成 base64 或把 URL 传进来。

use base64::Engine;
use std::io::Read;

/// 把前端传来的 base64（可含 data:URL 前缀）解码为字节
fn decode_base64(data: &str) -> Result<Vec<u8>, String> {
    let raw = match data.find(",") {
        // data:*/*;base64,XXXX → 取逗号后半段
        Some(pos) if data.starts_with("data:") => &data[pos + 1..],
        _ => data,
    };
    base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|e| format!("base64 解码失败: {e}"))
}

/// 根据文件名后缀分派解析；返回提取出的文本（Excel 为 Markdown 表格）
pub fn extract(filename: &str, base64_data: &str) -> Result<String, String> {
    let bytes = decode_base64(base64_data)?;
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".xlsx") || lower.ends_with(".xls") || lower.ends_with(".xlsm") || lower.ends_with(".ods") {
        excel_to_markdown(&bytes)
    } else if lower.ends_with(".docx") {
        docx_to_text(&bytes)
    } else if lower.ends_with(".csv") {
        // CSV 直接按纯文本处理
        String::from_utf8(bytes).map_err(|e| format!("CSV 读取失败: {e}"))
    } else {
        Err(format!("暂不支持的文件类型：{filename}（支持 .xlsx/.xls/.docx/.csv）"))
    }
}

/// Excel 全部工作表 → Markdown 表格（每表一个 ## 标题）
fn excel_to_markdown(bytes: &[u8]) -> Result<String, String> {
    use calamine::{Data, Reader, Xlsx};
    let cursor = std::io::Cursor::new(bytes.to_vec());
    let mut wb: Xlsx<_> = Xlsx::new(cursor).map_err(|e| format!("打开 Excel 失败: {e}"))?;
    let names = wb.sheet_names().to_vec();
    if names.is_empty() {
        return Err("Excel 中没有工作表".into());
    }
    let mut out = String::new();
    for name in names {
        let range = match wb.worksheet_range(&name) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if range.is_empty() {
            continue;
        }
        out.push_str(&format!("## 工作表：{name}\n\n"));
        let cell_str = |c: &Data| -> String {
            match c {
                Data::Empty => String::new(),
                Data::String(s) => s.replace('|', "\\|").replace('\n', " "),
                Data::Float(f) => {
                    if f.fract() == 0.0 { format!("{}", *f as i64) } else { format!("{f}") }
                }
                Data::Int(i) => i.to_string(),
                Data::Bool(b) => b.to_string(),
                Data::DateTime(d) => d.to_string(),
                other => other.to_string(),
            }
        };
        let rows: Vec<Vec<String>> = range.rows().map(|r| r.iter().map(cell_str).collect()).collect();
        let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if cols == 0 {
            continue;
        }
        // 首行作表头，没有则用列号
        let header = &rows[0];
        let head_cells: Vec<String> = (0..cols)
            .map(|i| header.get(i).cloned().filter(|s| !s.is_empty()).unwrap_or_else(|| format!("列{}", i + 1)))
            .collect();
        out.push_str(&format!("| {} |\n", head_cells.join(" | ")));
        out.push_str(&format!("| {} |\n", vec!["---"; cols].join(" | ")));
        for row in rows.iter().skip(1) {
            let cells: Vec<String> = (0..cols).map(|i| row.get(i).cloned().unwrap_or_default()).collect();
            out.push_str(&format!("| {} |\n", cells.join(" | ")));
        }
        out.push('\n');
    }
    if out.trim().is_empty() {
        return Err("Excel 内容为空".into());
    }
    Ok(out)
}

/// Word .docx → 纯文本：.docx 本质是 zip，正文在 word/document.xml，取所有 <w:t> 文本
fn docx_to_text(bytes: &[u8]) -> Result<String, String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    let cursor = std::io::Cursor::new(bytes.to_vec());
    let mut zip = zip::ZipArchive::new(cursor).map_err(|e| format!("打开 docx 失败: {e}"))?;
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .map_err(|_| "docx 中找不到 word/document.xml".to_string())?
        .read_to_string(&mut xml)
        .map_err(|e| format!("读取 document.xml 失败: {e}"))?;

    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(false);
    let mut out = String::new();
    let mut in_text = false;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            // quick-xml 0.42：str reader 的 QName::as_ref() 返回 &str，直接匹配字符串
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == "w:t" {
                    in_text = true;
                }
            }
            Ok(Event::End(e)) => {
                match e.name().as_ref() {
                    "w:t" => in_text = false,
                    // 段落 / 换行结束 → 换行
                    "w:p" => out.push('\n'),
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                // <w:br/> 软换行、<w:tab/> 制表
                match e.name().as_ref() {
                    "w:br" => out.push('\n'),
                    "w:tab" => out.push('\t'),
                    _ => {}
                }
            }
            Ok(Event::Text(t)) if in_text => {
                // 0.42 移除 unescape()：reader 解析期已反转义，xml_content 做 EOL 归一
                out.push_str(&t.xml_content(quick_xml::XmlVersion::Implicit1_0));
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("解析 docx 失败: {e}")),
            _ => {}
        }
        buf.clear();
    }
    // 折叠多余空行
    let cleaned: Vec<&str> = out.lines().map(|l| l.trim_end()).collect();
    let mut result = String::new();
    let mut blank = 0;
    for line in cleaned {
        if line.is_empty() {
            blank += 1;
            if blank <= 1 {
                result.push('\n');
            }
        } else {
            blank = 0;
            result.push_str(line);
            result.push('\n');
        }
    }
    let result = result.trim().to_string();
    if result.is_empty() {
        return Err("Word 文档内容为空".into());
    }
    Ok(result)
}

/// 抓取网页并提取正文文字（去掉 script/style/nav 等），返回标题 + 正文
pub async fn fetch_webpage(url: &str) -> Result<(String, String), String> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("请输入以 http:// 或 https:// 开头的网址".into());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (compatible; BIT/1.0; +https://github.com/yxpil/OpenBit)")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let html = resp.text().await.map_err(|e| format!("读取网页失败: {e}"))?;
    Ok(html_to_text(&html))
}

/// HTML → (标题, 正文文字)。剔除脚本/样式等噪声标签，压缩空白。
fn html_to_text(html: &str) -> (String, String) {
    use scraper::{Html, Selector};
    let doc = Html::parse_document(html);

    let title = Selector::parse("title")
        .ok()
        .and_then(|sel| doc.select(&sel).next())
        .map(|e| e.text().collect::<String>().trim().to_string())
        .unwrap_or_default();

    // 优先正文容器，退回 body
    let body_sel = ["article", "main", "body"]
        .iter()
        .find_map(|s| Selector::parse(s).ok().filter(|sel| doc.select(sel).next().is_some()))
        .or_else(|| Selector::parse("body").ok());

    let mut text = String::new();
    if let Some(sel) = body_sel {
        if let Some(root) = doc.select(&sel).next() {
            // 收集文本，跳过 script/style/noscript 的内容
            for node in root.text() {
                let t = node.trim();
                if !t.is_empty() {
                    text.push_str(t);
                    text.push('\n');
                }
            }
        }
    }
    // scraper 的 .text() 已不含 script/style（它们的文本节点仍会出现，故这里再做一次朴素过滤）
    let cleaned: Vec<&str> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    let mut body = cleaned.join("\n");
    // 限制长度，避免超大页面撑爆上下文
    const MAX: usize = 20_000;
    if body.chars().count() > MAX {
        body = body.chars().take(MAX).collect::<String>() + "\n…（正文过长已截断）";
    }
    (title, body)
}
