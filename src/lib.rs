use std::io::{Seek, SeekFrom, Write, self};


pub struct Pdf<'a, W: 'a + Write + Seek> {
    output: &'a mut W,
    object_offsets: Vec<i64>,
    page_objects_ids: Vec<usize>,
}

pub struct Canvas<'a, W: 'a + Write> {
    output: &'a mut W,
}

const ROOT_OBJECT_ID: usize = 1;
const PAGES_OBJECT_ID: usize = 2;

impl<'a, W: Write + Seek> Pdf<'a, W> {
    pub fn new(output: &'a mut W) -> io::Result<Pdf<'a, W>> {
        // FIXME: Find out the lowest version that contains the features we’re using.
        try!(output.write_all(b"%PDF-1.7\n%\xB5\xED\xAE\xFB\n"));
        Ok(Pdf {
            output: output,
            // Object ID 0 is special in PDF.
            // We reserve IDs 1 and 2 for the catalog and page tree.
            object_offsets: vec![-1, -1, -1],
            page_objects_ids: vec![],
        })
    }

    /// Return the current read/write position in the output file.
    fn tell(&mut self) -> io::Result<u64> {
        self.output.seek(SeekFrom::Current(0))
    }

    pub fn render_page<F>(&mut self, width: f32, height: f32, render_contents: F) -> io::Result<()>
    where F: FnOnce(&mut Canvas<W>) -> io::Result<()> {
        let (contents_object_id, content_length) =
        try!(self.write_new_object(move |contents_object_id, pdf| {
            // Guess the ID of the next object. (We’ll assert it below.)
            try!(write!(pdf.output, "<<  /Length {} 0 R\n", contents_object_id + 1));
            try!(write!(pdf.output, ">>\n"));
            try!(write!(pdf.output, "stream\n"));

            let start = try!(pdf.tell());
            try!(write!(pdf.output, "/DeviceRGB cs /DeviceRGB CS\n"));
            try!(write!(pdf.output, "0.75 0 0 -0.75 0 {} cm\n", height));
            try!(render_contents(&mut Canvas { output: pdf.output }));
            let end = try!(pdf.tell());

            try!(write!(pdf.output, "endstream\n"));
            Ok((contents_object_id, end - start))
        }));
        try!(self.write_new_object(|length_object_id, pdf| {
            assert!(length_object_id == contents_object_id + 1);
            write!(pdf.output, "{}\n", content_length)
        }));
        let page_object_id = try!(self.write_new_object(|page_object_id, pdf| {
            try!(write!(pdf.output, "<<  /Type /Page\n"));
            try!(write!(pdf.output, "    /Parent {} 0 R\n", PAGES_OBJECT_ID));
            try!(write!(pdf.output, "    /Resources << >>\n"));
            try!(write!(pdf.output, "    /MediaBox [ 0 0 {} {} ]\n", width, height));
            try!(write!(pdf.output, "    /Contents {} 0 R\n", contents_object_id));
            try!(write!(pdf.output, ">>\n"));
            Ok(page_object_id)
        }));
        self.page_objects_ids.push(page_object_id);
        Ok(())
    }

    fn write_new_object<F, T>(&mut self, write_content: F) -> io::Result<T>
    where F: FnOnce(usize, &mut Pdf<W>) -> io::Result<T> {
        let id = self.object_offsets.len();
        let (result, offset) = try!(self.write_object(id, |pdf| write_content(id, pdf)));
        self.object_offsets.push(offset);
        Ok(result)
    }

    fn write_object_with_id<F, T>(&mut self, id: usize, write_content: F) -> io::Result<T>
    where F: FnOnce(&mut Pdf<W>) -> io::Result<T> {
        assert!(self.object_offsets[id] == -1);
        let (result, offset) = try!(self.write_object(id, write_content));
        self.object_offsets[id] = offset;
        Ok(result)
    }

    fn write_object<F, T>(&mut self, id: usize, write_content: F) -> io::Result<(T, i64)>
    where F: FnOnce(&mut Pdf<W>) -> io::Result<T> {
        // `as i64` here would only overflow for PDF files bigger than 2**63 bytes
        let offset = try!(self.tell()) as i64;
        try!(write!(self.output, "{} 0 obj\n", id));
        let result = try!(write_content(self));
        try!(write!(self.output, "endobj\n"));
        Ok((result, offset))
    }

    pub fn finish(mut self) -> io::Result<()> {
        try!(self.write_object_with_id(PAGES_OBJECT_ID, |pdf| {
            try!(write!(pdf.output, "<<  /Type /Pages\n"));
            try!(write!(pdf.output, "    /Count {}\n", pdf.page_objects_ids.len()));
            try!(write!(pdf.output, "    /Kids [ "));
            for &page_object_id in &pdf.page_objects_ids {
                try!(write!(pdf.output, "{} 0 R ", page_object_id));
            }
            try!(write!(pdf.output, "]\n"));
            try!(write!(pdf.output, ">>\n"));
            Ok(())
        }));
        try!(self.write_object_with_id(ROOT_OBJECT_ID, |pdf| {
            try!(write!(pdf.output, "<<  /Type /Catalog\n"));
            try!(write!(pdf.output, "    /Pages {} 0 R\n", PAGES_OBJECT_ID));
            try!(write!(pdf.output, ">>\n"));
            Ok(())
        }));
        let startxref = try!(self.tell());
        try!(write!(self.output, "xref\n"));
        try!(write!(self.output, "0 {}\n", self.object_offsets.len()));
        // Object 0 is special
        try!(write!(self.output, "0000000000 65535 f \n"));
        // Use [1..] to skip object 0 in self.object_offsets.
        for &offset in &self.object_offsets[1..] {
            assert!(offset >= 0);
            try!(write!(self.output, "{:010} 00000 n \n", offset));
        }
        try!(write!(self.output, "trailer\n"));
        try!(write!(self.output, "<<  /Size {}\n", self.object_offsets.len()));
        try!(write!(self.output, "    /Root {} 0 R\n", ROOT_OBJECT_ID));
        try!(write!(self.output, ">>\n"));
        try!(write!(self.output, "startxref\n"));
        try!(write!(self.output, "{}\n", startxref));
        try!(write!(self.output, "%%EOF\n"));
        Ok(())
    }
}

impl<'a, W: Write> Canvas<'a, W> {
    pub fn rectangle(&mut self, r: u8, g: u8, b: u8, x: f32, y: f32, width: f32, height: f32)
                     -> io::Result<()> {
        write!(self.output, "{} {} {} sc {} {} {} {} re f\n",
               r, g, b,
               x, y, width, height)
    }
}
