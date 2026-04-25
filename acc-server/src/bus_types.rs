/// Declare every recognised bus MIME type in one place.
///
/// The `media_types!` macro expands to:
///   • An enum `MediaType` whose variants are listed here.
///   • `MediaType::from_mime(s: &str) -> Option<MediaType>` — parses the
///     canonical MIME string and returns the matching variant.
///   • `MediaType::as_mime(&self) -> &'static str` — the canonical MIME
///     string for a variant.
///   • `MediaType::is_binary(&self) -> bool` — true when the payload should
///     be treated as opaque bytes (base64-encoded on the wire).
///
/// Syntax: `media_types! { VariantName => "mime/string" [binary], … }`
/// The `binary` flag is optional; omitting it means the type is text.
macro_rules! media_types {
    (
        $( $variant:ident => $mime:literal $(, binary: $bin:tt)? );*
        $(;)?
    ) => {
        /// Every MIME type that the bus blob endpoint recognises.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum MediaType {
            $( $variant, )*
        }

        impl MediaType {
            /// Parse a MIME string and return the matching variant, or `None`
            /// if the string is not registered in the `media_types!` table.
            pub fn from_mime(s: &str) -> Option<MediaType> {
                match s {
                    $( $mime => Some(MediaType::$variant), )*
                    _ => None,
                }
            }

            /// Return the canonical MIME string for this variant.
            pub fn as_mime(&self) -> &'static str {
                match self {
                    $( MediaType::$variant => $mime, )*
                }
            }

            /// Return `true` when payload bytes should be treated as binary
            /// (i.e. base64-encoded on the bus wire format).
            pub fn is_binary(&self) -> bool {
                match self {
                    $(
                        MediaType::$variant => {
                            // The `binary` flag drives this arm.
                            // If no flag was given the value is `false`.
                            media_types!(@bin $( $bin )?)
                        }
                    )*
                }
            }
        }
    };

    // Helper: emit the boolean value for the `binary` flag.
    (@bin true)  => { true  };
    (@bin false) => { false };
    (@bin)       => { false };   // flag omitted → not binary
}

media_types! {
    // ── Text / structured ─────────────────────────────────────────────────
    TextPlain           => "text/plain";
    TextHtml            => "text/html";
    TextCsv             => "text/csv";
    TextMarkdown        => "text/markdown";
    ApplicationJson     => "application/json";
    ApplicationXml      => "application/xml";
    ApplicationYaml     => "application/yaml";

    // ── Images ───────────────────────────────────────────────────────────
    ImagePng            => "image/png",         binary: true;
    ImageJpeg           => "image/jpeg",        binary: true;
    ImageGif            => "image/gif",         binary: true;
    ImageWebp           => "image/webp",        binary: true;
    ImageSvgXml         => "image/svg+xml";
    ImageAvif           => "image/avif",        binary: true;

    // ── Audio ────────────────────────────────────────────────────────────
    AudioMpeg           => "audio/mpeg",        binary: true;
    AudioOgg            => "audio/ogg",         binary: true;
    AudioWav            => "audio/wav",         binary: true;
    AudioWebm           => "audio/webm",        binary: true;
    AudioFlac           => "audio/flac",        binary: true;
    AudioAac            => "audio/aac",         binary: true;

    // ── Video ────────────────────────────────────────────────────────────
    VideoMp4            => "video/mp4",         binary: true;
    VideoWebm           => "video/webm",        binary: true;
    VideoOgg            => "video/ogg",         binary: true;
    VideoMov            => "video/quicktime",   binary: true;

    // ── Binary / archives ─────────────────────────────────────────────────
    ApplicationOctetStream => "application/octet-stream", binary: true;
    ApplicationZip      => "application/zip",   binary: true;
    ApplicationPdf      => "application/pdf",   binary: true;

    // ── 3D model formats ─────────────────────────────────────────────────
    // glTF JSON (.gltf) — text-based, human-readable JSON scene descriptor.
    ModelGltfJson       => "model/gltf+json";
    // glTF Binary (.glb) — single-file binary container.
    ModelGltfBinary     => "model/gltf-binary", binary: true;
    // Wavefront OBJ (.obj) — plain-text geometry.
    ModelObj            => "model/obj";
    // Universal Scene Description Zip (.usdz) — Apple/Pixar binary package.
    ModelUsd            => "model/vnd.usdz+zip", binary: true;
    // STL (.stl) — typically binary (binary STL is the common interchange form).
    ModelStl            => "model/stl",         binary: true;
    // Stanford PLY (.ply) — may be ASCII or binary; treat as binary on the wire.
    ModelPly            => "model/ply",         binary: true;
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::MediaType;

    // ── round-trip: from_mime → as_mime ──────────────────────────────────────

    #[test]
    fn text_plain_round_trips() {
        let mt = MediaType::from_mime("text/plain").expect("text/plain must be recognised");
        assert_eq!(mt.as_mime(), "text/plain");
    }

    #[test]
    fn image_png_round_trips() {
        let mt = MediaType::from_mime("image/png").expect("image/png must be recognised");
        assert_eq!(mt.as_mime(), "image/png");
    }

    #[test]
    fn audio_ogg_round_trips() {
        let mt = MediaType::from_mime("audio/ogg").expect("audio/ogg must be recognised");
        assert_eq!(mt.as_mime(), "audio/ogg");
    }

    #[test]
    fn video_mp4_round_trips() {
        let mt = MediaType::from_mime("video/mp4").expect("video/mp4 must be recognised");
        assert_eq!(mt.as_mime(), "video/mp4");
    }

    // ── unknown MIME types return None ───────────────────────────────────────

    #[test]
    fn unknown_mime_returns_none() {
        assert!(MediaType::from_mime("application/x-unknown-type").is_none());
    }

    #[test]
    fn empty_mime_returns_none() {
        assert!(MediaType::from_mime("").is_none());
    }

    // ── is_binary classifications ─────────────────────────────────────────────

    #[test]
    fn text_plain_is_not_binary() {
        assert!(!MediaType::TextPlain.is_binary());
    }

    #[test]
    fn application_json_is_not_binary() {
        assert!(!MediaType::ApplicationJson.is_binary());
    }

    #[test]
    fn image_png_is_binary() {
        assert!(MediaType::ImagePng.is_binary());
    }

    #[test]
    fn audio_mpeg_is_binary() {
        assert!(MediaType::AudioMpeg.is_binary());
    }

    #[test]
    fn video_mp4_is_binary() {
        assert!(MediaType::VideoMp4.is_binary());
    }

    #[test]
    fn application_zip_is_binary() {
        assert!(MediaType::ApplicationZip.is_binary());
    }

    // ── 3D model variants: from_mime ─────────────────────────────────────────

    #[test]
    fn model_gltf_json_from_mime() {
        let mt = MediaType::from_mime("model/gltf+json")
            .expect("model/gltf+json must be recognised");
        assert_eq!(mt, MediaType::ModelGltfJson);
        assert_eq!(mt.as_mime(), "model/gltf+json");
    }

    #[test]
    fn model_gltf_binary_from_mime() {
        let mt = MediaType::from_mime("model/gltf-binary")
            .expect("model/gltf-binary must be recognised");
        assert_eq!(mt, MediaType::ModelGltfBinary);
        assert_eq!(mt.as_mime(), "model/gltf-binary");
    }

    #[test]
    fn model_obj_from_mime() {
        let mt = MediaType::from_mime("model/obj")
            .expect("model/obj must be recognised");
        assert_eq!(mt, MediaType::ModelObj);
        assert_eq!(mt.as_mime(), "model/obj");
    }

    #[test]
    fn model_usd_from_mime() {
        let mt = MediaType::from_mime("model/vnd.usdz+zip")
            .expect("model/vnd.usdz+zip must be recognised");
        assert_eq!(mt, MediaType::ModelUsd);
        assert_eq!(mt.as_mime(), "model/vnd.usdz+zip");
    }

    #[test]
    fn model_stl_from_mime() {
        let mt = MediaType::from_mime("model/stl")
            .expect("model/stl must be recognised");
        assert_eq!(mt, MediaType::ModelStl);
        assert_eq!(mt.as_mime(), "model/stl");
    }

    #[test]
    fn model_ply_from_mime() {
        let mt = MediaType::from_mime("model/ply")
            .expect("model/ply must be recognised");
        assert_eq!(mt, MediaType::ModelPly);
        assert_eq!(mt.as_mime(), "model/ply");
    }

    // ── 3D model variants: is_binary ─────────────────────────────────────────
    //
    // Text-format types (is_binary == false):
    //   model/gltf+json  — human-readable JSON scene descriptor
    //   model/obj        — plain-text Wavefront geometry
    //   model/vrml       — plain-text ASCII VRML 97 scene
    //
    // Binary-format types (is_binary == true):
    //   model/gltf-binary  — single-file GLB container
    //   model/vnd.usdz+zip — Apple/Pixar USDZ zip archive
    //   model/stl          — binary STL (common interchange form)
    //   model/ply          — PLY treated as binary on the wire

    /// glTF JSON is a text format — it must NOT be flagged as binary.
    #[test]
    fn model_gltf_json_is_not_binary() {
        assert!(!MediaType::ModelGltfJson.is_binary(),
            "model/gltf+json is a text-based JSON format and must not be is_binary");
    }

    /// GLB is a binary container — it MUST be flagged as binary.
    #[test]
    fn model_gltf_binary_is_binary() {
        assert!(MediaType::ModelGltfBinary.is_binary(),
            "model/gltf-binary is a binary format and must be is_binary");
    }

    /// OBJ is plain-text — it must NOT be flagged as binary.
    #[test]
    fn model_obj_is_not_binary() {
        assert!(!MediaType::ModelObj.is_binary(),
            "model/obj is a text format and must not be is_binary");
    }

    /// USDZ is a zip archive — it MUST be flagged as binary.
    #[test]
    fn model_usd_is_binary() {
        assert!(MediaType::ModelUsd.is_binary(),
            "model/vnd.usdz+zip is a binary zip archive and must be is_binary");
    }

    /// STL (binary form) must be flagged as binary.
    #[test]
    fn model_stl_is_binary() {
        assert!(MediaType::ModelStl.is_binary(),
            "model/stl is treated as binary on the bus wire and must be is_binary");
    }

    /// PLY is treated as binary on the bus wire.
    #[test]
    fn model_ply_is_binary() {
        assert!(MediaType::ModelPly.is_binary(),
            "model/ply is treated as binary on the bus wire and must be is_binary");
    }
}
