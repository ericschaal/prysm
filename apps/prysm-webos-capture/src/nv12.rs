//! The webOS ARM build targets NEON. Other widths use the portable yuv converter.
use rayon::prelude::*;

unsafe extern "C" {
    fn prysm_nv12_rows(y: *const u8, uv: *const u8, rgb: *mut u8, width: usize, pairs: usize);
}

pub(super) fn convert(y: &[u8], uv: &[u8], rgb: &mut [u8], width: usize) {
    assert!((8..=1920).contains(&width) && width.is_multiple_of(8));
    assert!(uv.len().is_multiple_of(width));
    assert_eq!(y.len(), 2 * uv.len());
    assert_eq!(Some(rgb.len()), y.len().checked_mul(3));

    // Keep adjacent row pairs together to amortize dispatch and reuse chroma.
    let band = 8 * width;
    rgb.par_chunks_mut(6 * band)
        .zip(y.par_chunks(2 * band))
        .zip(uv.par_chunks(band))
        .for_each(|((out, y), uv)| {
            // SAFETY: the checks above give each band complete, non-overlapping
            // rows with eight-pixel blocks and one UV row per two Y rows.
            unsafe {
                prysm_nv12_rows(
                    y.as_ptr(),
                    uv.as_ptr(),
                    out.as_mut_ptr(),
                    width,
                    uv.len() / width,
                );
            }
        });
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_y_u_v_combination_matches_the_portable_converter() {
        const WIDTH: usize = 256;
        const HEIGHT: usize = 514; // Includes a partial final band.
        let width = u32::try_from(WIDTH).unwrap();
        let height = u32::try_from(HEIGHT).unwrap();
        let y: Vec<_> = (0..HEIGHT)
            .flat_map(|row| (0..=u8::MAX).map(move |x| if row % 2 == 0 { x } else { !x }))
            .collect();
        let mut uv = vec![0; WIDTH * HEIGHT / 2];
        let mut expected = vec![0; WIDTH * HEIGHT * 3];
        let mut actual = expected.clone();
        for u in 0..=u8::MAX {
            for (row, v) in uv
                .as_chunks_mut::<WIDTH>()
                .0
                .iter_mut()
                .zip((0..=u8::MAX).cycle())
            {
                for pair in row.as_chunks_mut::<2>().0 {
                    *pair = [u, v];
                }
            }
            yuv::yuv_nv12_to_rgb(
                &yuv::YuvBiPlanarImage {
                    y_plane: &y,
                    y_stride: width,
                    uv_plane: &uv,
                    uv_stride: width,
                    width,
                    height,
                },
                &mut expected,
                width * 3,
                yuv::YuvRange::Limited,
                yuv::YuvStandardMatrix::Bt709,
                yuv::YuvConversionMode::Balanced,
            )
            .unwrap();
            super::convert(&y, &uv, &mut actual, WIDTH);
            assert!(actual == expected, "conversion differs at U={u}");
        }
    }
}
