//! Icône de Patronus : une tête de cerf argentée, lumineuse, comme un patronus. Partagée par
//! l'application (icône de fenêtre) et `build.rs` (icônes de l'exécutable) : aucune dépendance.
//!
//! Le dessin n'est fait que de disques étirés (ellipses, éventuellement tournées) et de segments
//! épais (bois du cerf), sur une toile de 100 × 100 unités : grossir chaque forme d'un même rayon
//! donne le contour, sans calcul de décalage de polygone.

/// Forme élémentaire.
enum Shape {
    /// Ellipse : centre, demi-axes, angle (radians).
    Ellipse((f32, f32), f32, f32, f32),
    /// Segment épais : extrémités, rayon.
    Stroke((f32, f32), (f32, f32), f32),
}

impl Shape {
    /// Le point est-il dans la forme grossie de `grow` unités ?
    fn contains(&self, x: f32, y: f32, grow: f32) -> bool {
        match *self {
            Shape::Ellipse((cx, cy), rx, ry, a) => {
                let (s, c) = a.sin_cos();
                let (dx, dy) = (x - cx, y - cy);
                let (u, v) = (dx * c + dy * s, -dx * s + dy * c);
                (u / (rx + grow)).powi(2) + (v / (ry + grow)).powi(2) <= 1.0
            }
            Shape::Stroke((ax, ay), (bx, by), r) => {
                let (vx, vy) = (bx - ax, by - ay);
                let t = (((x - ax) * vx + (y - ay) * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
                let (px, py) = (ax + t * vx - x, ay + t * vy - y);
                px * px + py * py <= (r + grow).powi(2)
            }
        }
    }
}

/// Une moitié des bois (la gauche) ; la droite en est le miroir.
const ANTLER: [((f32, f32), (f32, f32)); 6] = [
    ((43.0, 42.0), (38.5, 30.0)),
    ((38.5, 30.0), (31.5, 19.5)),
    ((31.5, 19.5), (29.0, 6.5)),
    ((38.0, 31.0), (27.0, 28.5)),
    ((33.5, 22.0), (37.5, 10.5)),
    ((30.0, 13.5), (21.5, 9.0)),
];

fn head() -> Vec<Shape> {
    let mut s = vec![
        Shape::Ellipse((50.0, 54.0), 11.5, 14.0, 0.0),
        Shape::Stroke((50.0, 60.0), (50.0, 73.0), 8.0),
        Shape::Stroke((50.0, 73.0), (50.0, 80.0), 5.8),
        Shape::Ellipse((34.5, 47.0), 9.0, 3.8, -0.38),
        Shape::Ellipse((65.5, 47.0), 9.0, 3.8, 0.38),
    ];
    for (a, b) in ANTLER {
        s.push(Shape::Stroke(a, b, 2.6));
        s.push(Shape::Stroke((100.0 - a.0, a.1), (100.0 - b.0, b.1), 2.6));
    }
    s
}

fn marks() -> Vec<Shape> {
    vec![
        Shape::Ellipse((44.5, 52.0), 1.9, 2.3, 0.0),
        Shape::Ellipse((55.5, 52.0), 1.9, 2.3, 0.0),
        Shape::Ellipse((50.0, 79.5), 3.4, 2.4, 0.0),
    ]
}

/// Image RGBA (non prémultipliée) de `size` × `size` pixels.
pub fn render(size: u32) -> Vec<u8> {
    // Suréchantillonnage 4 × 4 pour des bords lissés ; contour plus épais en petite taille.
    const SS: usize = 4;
    let size = size as usize;
    let outline = if size <= 24 { 3.5 } else { 2.4 };
    let (head, marks) = (head(), marks());
    let mut out = vec![0u8; size * size * 4];
    for py in 0..size {
        for px in 0..size {
            let (mut fill, mut edge, mut mark) = (0u32, 0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) * 100.0 / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) * 100.0 / size as f32;
                    if head.iter().any(|s| s.contains(x, y, 0.0)) {
                        if size > 20 && marks.iter().any(|s| s.contains(x, y, 0.0)) {
                            mark += 1;
                        } else {
                            fill += 1;
                        }
                    } else if head.iter().any(|s| s.contains(x, y, outline)) {
                        edge += 1;
                    }
                }
            }
            let total = fill + edge + mark;
            if total == 0 {
                continue;
            }
            // Lueur : blanc bleuté au centre du front, bleu vers les bords.
            let (cx, cy) = ((px as f32 + 0.5) * 100.0 / size as f32, (py as f32 + 0.5) * 100.0 / size as f32);
            let t = (((cx - 50.0).powi(2) + (cy - 45.0).powi(2)).sqrt() / 48.0).min(1.0);
            let glow = [236.0 - 96.0 * t, 246.0 - 56.0 * t, 255.0];
            let colors = [(fill, glow), (edge, [28.0, 72.0, 160.0]), (mark, [30.0, 64.0, 140.0])];
            let mut c = [0.0f32; 3];
            for (n, col) in colors {
                for i in 0..3 {
                    c[i] += col[i] * n as f32;
                }
            }
            let o = (py * size + px) * 4;
            for i in 0..3 {
                out[o + i] = (c[i] / total as f32).round() as u8;
            }
            out[o + 3] = ((total as f32 / (SS * SS) as f32) * 255.0).round() as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_a_transparent_background_and_an_opaque_head() {
        let img = render(64);
        assert_eq!(img.len(), 64 * 64 * 4);
        let alpha = |x: usize, y: usize| img[(y * 64 + x) * 4 + 3];
        assert_eq!(alpha(1, 62), 0, "coin transparent");
        assert_eq!(alpha(32, 40), 255, "front opaque");
        // Symétrique : les bois gauche et droit se répondent.
        for (x, y) in [(18, 14), (20, 9), (14, 6)] {
            assert_eq!(alpha(x, y) > 0, alpha(63 - x, y) > 0, "({x}, {y})");
        }
    }
}
