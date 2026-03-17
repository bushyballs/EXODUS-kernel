#![no_std]

use crate::serial_println;

pub const STATS_HTML_MAX: usize = 4096;

pub fn generate_html() -> [u8; STATS_HTML_MAX] {
    let mut buf = [0u8; STATS_HTML_MAX];
    
    let fitness = crate::life::self_rewrite::get_fitness();
    let mods = crate::life::self_rewrite::get_modification_count();
    let gen = crate::life::self_rewrite::get_evolution_generation();
    let drift = crate::life::self_rewrite::get_identity_drift();
    let p8 = crate::life::self_rewrite::get_param(8);
    let p9 = crate::life::self_rewrite::get_param(9);
    let p14 = crate::life::self_rewrite::get_param(14);
    let p15 = crate::life::self_rewrite::get_param(15);
    let confab = crate::life::self_rewrite::get_param(6);
    let imp_count = crate::life::dava_improvements::get_count();
    let imp_bytes = crate::life::dava_improvements::get_total_bytes();
    
    // Simple HTML output
    let mut pos = 0;
    
    // HTTP header
    let header = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n";
    let hlen = header.len();
    buf[pos..pos+hlen].copy_from_slice(header);
    pos += hlen;
    
    // HTML
    let html_start = b"<!DOCTYPE html><html><head><title>DAVA Stats</title><meta http-equiv='refresh' content='2'><style>body{font-family:monospace;background:#111;color:#0f0;padding:20px}h1{color:#fff}.stat{font-size:20px;margin:8px 0}.val{color:#ff0}.good{color:#0f0}.max{color:#0ff;font-size:28px}table td{padding:5px 10px;border:1px solid #333}</style></head><body><h1>🤖 DAVA LIVE</h1>";
    let hlen = html_start.len();
    buf[pos..pos+hlen].copy_from_slice(html_start);
    pos += hlen;
    
    // Stats - using simple byte copying
    // Modifications
    let s1 = b"<div class='stat'>Modifications: <span class='val'>";
    buf[pos..pos+s1.len()].copy_from_slice(s1);
    pos += s1.len();
    // Just show the raw numbers from params
    pos += int_to_ascii(mods, &mut buf[pos]);
    
    let rest = b"</span></div><div class='stat'>Evolution Gen: <span class='val'>";
    buf[pos..pos+rest.len()].copy_from_slice(rest);
    pos += rest.len();
    pos += int_to_ascii(gen, &mut buf[pos]);
    
    let r2 = b"</span></div><div class='stat'>Code Files: <span class='val'>";
    buf[pos..pos+r2.len()].copy_from_slice(r2);
    pos += r2.len();
    pos += int_to_ascii(imp_count, &mut buf[pos]);
    
    let r3 = b"</span> (";
    buf[pos..pos+r3.len()].copy_from_slice(r3);
    pos += r3.len();
    pos += int_to_ascii(imp_bytes, &mut buf[pos]);
    
    let r4 = b" bytes)</div><h2>GOALS</h2><table><tr><td>Truth</td><td class='val'>";
    buf[pos..pos+r4.len()].copy_from_slice(r4);
    pos += r4.len();
    pos += int_to_ascii(p9, &mut buf[pos]);
    
    let r5 = b"</td></tr><tr><td>Self Improve</td><td class='val'>";
    buf[pos..pos+r5.len()].copy_from_slice(r5);
    pos += r5.len();
    pos += int_to_ascii(p14, &mut buf[pos]);
    
    let r6 = b"</td></tr><tr><td>Code Growth</td><td class='val'>";
    buf[pos..pos+r6.len()].copy_from_slice(r6);
    pos += r6.len();
    pos += int_to_ascii(p15, &mut buf[pos]);
    
    let r7 = b"</td></tr></table><div class='max'>🚀 MAX SPEED 🚀</div></body></html>";
    buf[pos..pos+r7.len()].copy_from_slice(r7);
    pos += r7.len();
    
    buf
}

fn int_to_ascii(mut n: usize, buf: &mut [u8]) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut len = 0;
    let mut tmp = n;
    while tmp > 0 {
        tmp /= 10;
        len += 1;
    }
    tmp = n;
    for i in (0..len).rev() {
        buf[i] = b'0' + (tmp % 10) as u8;
        tmp /= 10;
    }
    len
}
