pub fn render_icon_rgba(size: u32) -> (Vec<u8>, u32, u32) {
    const SS: u32 = 4;
    let s = size.max(1);
    let k = s as f32 / 32.0;
    let mut buf = vec![0u8; (s * s * 4) as usize];

    let round_rect = |px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32| -> bool {
        if px < x0 || px > x1 || py < y0 || py > y1 {
            return false;
        }
        if (px >= x0 + r && px <= x1 - r) || (py >= y0 + r && py <= y1 - r) {
            return true;
        }
        let cx = px.clamp(x0 + r, x1 - r);
        let cy = py.clamp(y0 + r, y1 - r);
        let dx = px - cx;
        let dy = py - cy;
        dx * dx + dy * dy <= r * r
    };

    let in_tri =
        |px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32| -> bool {
            let s1 = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
            let s2 = (cx - bx) * (py - by) - (cy - by) * (px - bx);
            let s3 = (ax - cx) * (py - cy) - (ay - cy) * (px - cx);
            (s1 >= 0. && s2 >= 0. && s3 >= 0.) || (s1 <= 0. && s2 <= 0. && s3 <= 0.)
        };

    let circle = |px: f32, py: f32, cx: f32, cy: f32, r: f32| -> bool {
        let dx = px - cx;
        let dy = py - cy;
        dx * dx + dy * dy <= r * r
    };

    let (bx0, by0, bx1, by1, br) = (8.0 * k, 9.5 * k, 24.0 * k, 20.5 * k, 4.5 * k);
    let (t0x, t0y, t1x, t1y, t2x, t2y) = (
        11.0 * k, 20.0 * k, 14.5 * k, 26.0 * k, 17.0 * k, 20.0 * k,
    );

    for y in 0..s {
        for x in 0..s {
            let mut a_blue = 0u32;
            let mut a_white = 0u32;
            let mut a_dot = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = ((x * SS + sx) as f32 + 0.5) / SS as f32;
                    let py = ((y * SS + sy) as f32 + 0.5) / SS as f32;
                    if round_rect(px, py, 1.0 * k, 1.0 * k, 31.0 * k, 31.0 * k, 9.0 * k) {
                        a_blue += 1;
                    }
                    let bubble = round_rect(px, py, bx0, by0, bx1, by1, br);
                    let tail = in_tri(px, py, t0x, t0y, t1x, t1y, t2x, t2y);
                    if bubble || tail {
                        a_white += 1;
                    }
                    if circle(px, py, 12.0 * k, 15.0 * k, 1.6 * k)
                        || circle(px, py, 16.0 * k, 15.0 * k, 1.6 * k)
                        || circle(px, py, 20.0 * k, 15.0 * k, 1.6 * k)
                    {
                        a_dot += 1;
                    }
                }
            }
            let total = (SS * SS) as f32;
            let blue = a_blue as f32 / total;
            let white = a_white as f32 / total;
            let dot = a_dot as f32 / total;

            let (mut r, mut g, mut b, mut a) = (0f32, 0f32, 0f32, 0f32);
            if blue > 0.0 {
                r = 61.0;
                g = 90.0;
                b = 254.0;
                a = blue;
            }
            if white > 0.0 {
                r = r * (1.0 - white) + 255.0 * white;
                g = g * (1.0 - white) + 255.0 * white;
                b = b * (1.0 - white) + 255.0 * white;
                a = a.max(white);
            }
            if dot > 0.0 {
                r = r * (1.0 - dot) + 61.0 * dot;
                g = g * (1.0 - dot) + 90.0 * dot;
                b = b * (1.0 - dot) + 254.0 * dot;
                a = a.max(dot);
            }
            let i = ((y * s + x) * 4) as usize;
            buf[i] = r as u8;
            buf[i + 1] = g as u8;
            buf[i + 2] = b as u8;
            buf[i + 3] = (a * 255.0) as u8;
        }
    }
    (buf, s, s)
}