//! Deterministic, two-page vector PDF used only by tests / offline rendering.
//! No private document, file attachment, action, URL, or font download.
pub fn document(rotation: i32, cropped: bool) -> Vec<u8> {
    let (media, crop, translation) = if cropped {
        ("[0 0 400 500]", "[50 70 250 370]", "1 0 0 1 50 70 cm\n")
    } else {
        ("[0 0 200 300]", "[0 0 200 300]", "")
    };
    let content = format!(
        "q\n{translation}1 0 0 rg 0 240 200 60 re f\n0 0 1 rg 0 0 200 60 re f\n0 0 0 rg BT /F1 18 Tf 30 170 Td (PasteRS) Tj 0 -25 Td (First page) Tj ET\nQ\n"
    );
    let second = "0 1 0 rg 0 0 200 300 re f\n";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".into(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox {media} /CropBox {crop} /Rotate {rotation} /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
        ),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Resources << >> /Contents 7 0 R >>"
            .into(),
        format!("<< /Length {} >>\nstream\n{second}endstream", second.len()),
    ];
    let mut pdf = "%PDF-1.4\n".to_owned();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let xref = pdf.len();
    pdf.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    pdf.into_bytes()
}
