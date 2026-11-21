// LibRaw 0.22.3 (CDDL-1.0, chosen from its LGPL-2.1/CDDL-1.0 dual license) is built from the
// unmodified sources in third_party and linked statically. It only runs inside the isolated
// RAW worker process (see src/libraw.rs), never in the viewer process itself.
const LIBRAW: &str = "../../third_party/LibRaw-0.22.3";
const SOURCES: &[&str] = &[
    "src/decoders/canon_600.cpp", "src/decoders/crx.cpp", "src/decoders/decoders_dcraw.cpp",
    "src/decoders/decoders_libraw.cpp", "src/decoders/decoders_libraw_dcrdefs.cpp", "src/decoders/dng.cpp",
    "src/decoders/fp_dng.cpp", "src/decoders/fuji_compressed.cpp", "src/decoders/generic.cpp",
    "src/decoders/kodak_decoders.cpp", "src/decoders/load_mfbacks.cpp", "src/decoders/olympus14.cpp",
    "src/decoders/pana8.cpp", "src/decoders/smal.cpp", "src/decoders/sonycc.cpp", "src/decoders/unpack.cpp",
    "src/decoders/unpack_thumb.cpp", "src/decompressors/losslessjpeg.cpp", "src/demosaic/aahd_demosaic.cpp",
    "src/demosaic/ahd_demosaic.cpp", "src/demosaic/dcb_demosaic.cpp", "src/demosaic/dht_demosaic.cpp",
    "src/demosaic/misc_demosaic.cpp", "src/demosaic/xtrans_demosaic.cpp", "src/integration/dngsdk_glue.cpp",
    "src/integration/rawspeed_glue.cpp", "src/libraw_c_api.cpp", "src/libraw_datastream.cpp",
    "src/metadata/adobepano.cpp", "src/metadata/canon.cpp", "src/metadata/ciff.cpp", "src/metadata/cr3_parser.cpp",
    "src/metadata/epson.cpp", "src/metadata/exif_gps.cpp", "src/metadata/fuji.cpp", "src/metadata/hasselblad_model.cpp",
    "src/metadata/identify.cpp", "src/metadata/identify_tools.cpp", "src/metadata/kodak.cpp", "src/metadata/leica.cpp",
    "src/metadata/makernotes.cpp", "src/metadata/mediumformat.cpp", "src/metadata/minolta.cpp",
    "src/metadata/misc_parsers.cpp", "src/metadata/nikon.cpp", "src/metadata/normalize_model.cpp",
    "src/metadata/olympus.cpp", "src/metadata/p1.cpp", "src/metadata/pentax.cpp", "src/metadata/samsung.cpp",
    "src/metadata/sony.cpp", "src/metadata/tiff.cpp", "src/postprocessing/aspect_ratio.cpp",
    "src/postprocessing/dcraw_process.cpp", "src/postprocessing/mem_image.cpp", "src/postprocessing/postprocessing_aux.cpp",
    "src/postprocessing/postprocessing_utils.cpp", "src/postprocessing/postprocessing_utils_dcrdefs.cpp",
    "src/preprocessing/ext_preprocess.cpp", "src/preprocessing/raw2image.cpp", "src/preprocessing/subtract_black.cpp",
    "src/tables/cameralist.cpp", "src/tables/colorconst.cpp", "src/tables/colordata.cpp", "src/tables/wblists.cpp",
    "src/utils/curves.cpp", "src/utils/decoder_info.cpp", "src/utils/init_close_utils.cpp", "src/utils/open.cpp",
    "src/utils/phaseone_processing.cpp", "src/utils/read_utils.cpp", "src/utils/thumb_utils.cpp",
    "src/utils/utils_dcraw.cpp", "src/utils/utils_libraw.cpp", "src/write/apply_profile.cpp",
    "src/write/file_write.cpp", "src/write/tiff_writer.cpp", "src/x3f/x3f_parse_process.cpp",
    "src/x3f/x3f_utils_patched.cpp",
];

fn main() {
    let mut build = cc::Build::new();
    build.cpp(true).include(LIBRAW).define("LIBRAW_NODLL", None).define("LIBRAW_BUILDLIB", None)
        .define("NO_JASPER", None).warnings(false).file("src/libraw_shim.cpp");
    if build.get_compiler().is_like_msvc() {
        build.flag("/EHsc").define("WIN32", None).define("_CRT_SECURE_NO_WARNINGS", None);
    }
    for source in SOURCES { build.file(format!("{LIBRAW}/{source}")); }
    build.compile("clipview_libraw");
    println!("cargo:rerun-if-changed=src/libraw_shim.cpp");
    println!("cargo:rerun-if-changed={LIBRAW}");
    tauri_build::build();
}
