use core::fmt::Debug;
use core::{iter, result, slice, str};

use crate::{
    xcoff, BigEndian as BE, CompressedData, CompressedFileRange, Pod, SectionFlags, SectionKind,
};

use crate::read::{self, ObjectSection, ReadError, ReadRef, Result, SectionIndex};

use super::{AuxHeader, FileHeader, XcoffFile, XcoffRelocationIterator};

/// An iterator over the sections of an `XcoffFile32`.
pub type XcoffSectionIterator32<'data, 'file, R = &'data [u8]> =
    XcoffSectionIterator<'data, 'file, xcoff::FileHeader32, R>;
/// An iterator over the sections of an `XcoffFile64`.
pub type XcoffSectionIterator64<'data, 'file, R = &'data [u8]> =
    XcoffSectionIterator<'data, 'file, xcoff::FileHeader64, R>;

/// An iterator over the sections of an `XcoffFile`.
#[derive(Debug)]
pub struct XcoffSectionIterator<'data, 'file, Xcoff, R = &'data [u8]>
where
    Xcoff: FileHeader,
    R: ReadRef<'data>,
{
    pub(super) file: &'file XcoffFile<'data, Xcoff, R>,
    pub(super) iter: iter::Enumerate<slice::Iter<'data, Xcoff::SectionHeader>>,
}

impl<'data, 'file, Xcoff, R> Iterator for XcoffSectionIterator<'data, 'file, Xcoff, R>
where
    Xcoff: FileHeader,
    R: ReadRef<'data>,
{
    type Item = XcoffSection<'data, 'file, Xcoff, R>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|(index, section)| XcoffSection {
            index: SectionIndex(index),
            file: self.file,
            section,
        })
    }
}

/// A section of an `XcoffFile32`.
pub type XcoffSection32<'data, 'file, R = &'data [u8]> =
    XcoffSection<'data, 'file, xcoff::FileHeader32, R>;
/// A section of an `XcoffFile64`.
pub type XcoffSection64<'data, 'file, R = &'data [u8]> =
    XcoffSection<'data, 'file, xcoff::FileHeader64, R>;

/// A section of an `XcoffFile`.
#[derive(Debug)]
pub struct XcoffSection<'data, 'file, Xcoff, R = &'data [u8]>
where
    'data: 'file,
    Xcoff: FileHeader,
    R: ReadRef<'data>,
{
    pub(super) file: &'file XcoffFile<'data, Xcoff, R>,
    pub(super) section: &'data Xcoff::SectionHeader,
    pub(super) index: SectionIndex,
}

impl<'data, 'file, Xcoff: FileHeader, R: ReadRef<'data>> XcoffSection<'data, 'file, Xcoff, R> {
    fn bytes(&self) -> Result<&'data [u8]> {
        self.section
            .data(self.file.data)
            .read_error("Invalid XCOFF section offset or size")
    }
}

impl<'data, 'file, Xcoff, R> read::private::Sealed for XcoffSection<'data, 'file, Xcoff, R>
where
    Xcoff: FileHeader,
    R: ReadRef<'data>,
{
}

impl<'data, 'file, Xcoff, R> ObjectSection<'data> for XcoffSection<'data, 'file, Xcoff, R>
where
    Xcoff: FileHeader,
    R: ReadRef<'data>,
{
    type RelocationIterator = XcoffRelocationIterator<'data, 'file, Xcoff, R>;

    fn index(&self) -> SectionIndex {
        self.index
    }

    fn address(&self) -> u64 {
        self.section.s_paddr().into()
    }

    fn size(&self) -> u64 {
        self.section.s_size().into()
    }

    fn align(&self) -> u64 {
        // The default section alignment is 4.
        if let Some(aux_header) = self.file.aux_header {
            match self.kind() {
                SectionKind::Text => aux_header.o_algntext().into(),
                SectionKind::Data => aux_header.o_algndata().into(),
                _ => 4,
            }
        } else {
            4
        }
    }

    fn file_range(&self) -> Option<(u64, u64)> {
        self.section.file_range()
    }

    fn data(&self) -> Result<&'data [u8]> {
        self.bytes()
    }

    fn data_range(&self, address: u64, size: u64) -> Result<Option<&'data [u8]>> {
        Ok(read::util::data_range(
            self.bytes()?,
            self.address(),
            address,
            size,
        ))
    }

    fn compressed_file_range(&self) -> Result<CompressedFileRange> {
        Ok(CompressedFileRange::none(self.file_range()))
    }

    fn compressed_data(&self) -> Result<CompressedData<'data>> {
        self.data().map(CompressedData::none)
    }

    fn name_bytes(&self) -> read::Result<&[u8]> {
        Ok(self.section.name())
    }

    fn name(&self) -> read::Result<&str> {
        let name = self.name_bytes()?;
        str::from_utf8(name)
            .ok()
            .read_error("Non UTF-8 XCOFF section name")
    }

    fn segment_name_bytes(&self) -> Result<Option<&[u8]>> {
        Ok(None)
    }

    fn segment_name(&self) -> Result<Option<&str>> {
        Ok(None)
    }

    fn kind(&self) -> SectionKind {
        let section_type = self.section.s_flags() as u16;
        if section_type & xcoff::STYP_TEXT != 0 {
            SectionKind::Text
        } else if section_type & xcoff::STYP_DATA != 0 {
            SectionKind::Data
        } else if section_type & xcoff::STYP_TDATA != 0 {
            SectionKind::Tls
        } else if section_type & xcoff::STYP_BSS != 0 {
            SectionKind::UninitializedData
        } else if section_type & xcoff::STYP_TBSS != 0 {
            SectionKind::UninitializedTls
        } else if section_type & (xcoff::STYP_DEBUG | xcoff::STYP_DWARF) != 0 {
            SectionKind::Debug
        } else if section_type & (xcoff::STYP_LOADER | xcoff::STYP_OVRFLO) != 0 {
            SectionKind::Metadata
        } else if section_type
            & (xcoff::STYP_INFO | xcoff::STYP_EXCEPT | xcoff::STYP_PAD | xcoff::STYP_TYPCHK)
            != 0
        {
            SectionKind::Other
        } else {
            SectionKind::Unknown
        }
    }

    fn relocations(&self) -> Self::RelocationIterator {
        XcoffRelocationIterator { file: self.file }
    }

    fn flags(&self) -> SectionFlags {
        SectionFlags::Xcoff {
            s_flags: self.section.s_flags().into(),
        }
    }

    fn uncompressed_data(&self) -> Result<alloc::borrow::Cow<'data, [u8]>> {
        self.compressed_data()?.decompress()
    }
}

/// The table of section headers in an XCOFF file.
#[derive(Debug, Clone, Copy)]
pub struct SectionTable<'data, Xcoff: FileHeader> {
    sections: &'data [Xcoff::SectionHeader],
}

impl<'data, Xcoff> Default for SectionTable<'data, Xcoff>
where
    Xcoff: FileHeader,
{
    fn default() -> Self {
        Self { sections: &[] }
    }
}

impl<'data, Xcoff> SectionTable<'data, Xcoff>
where
    Xcoff: FileHeader,
{
    /// Parse the section table.
    ///
    /// `data` must be the entire file data.
    /// `offset` must be after the optional file header.
    pub fn parse<R: ReadRef<'data>>(header: &Xcoff, data: R, offset: &mut u64) -> Result<Self> {
        let section_num = header.f_nscns();
        if section_num == 0 {
            return Ok(SectionTable::default());
        }
        let sections = data
            .read_slice(offset, section_num as usize)
            .read_error("Invalid XCOFF section headers")?;
        Ok(SectionTable { sections })
    }

    /// Iterate over the section headers.
    #[inline]
    pub fn iter(&self) -> slice::Iter<'data, Xcoff::SectionHeader> {
        self.sections.iter()
    }

    /// Return true if the section table is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// The number of section headers.
    #[inline]
    pub fn len(&self) -> usize {
        self.sections.len()
    }

    /// Return the section header at the given index.
    pub fn section(&self, index: SectionIndex) -> read::Result<&'data Xcoff::SectionHeader> {
        self.sections
            .get(index.0)
            .read_error("Invalid XCOFF section index")
    }
}

/// A trait for generic access to `SectionHeader32` and `SectionHeader64`.
#[allow(missing_docs)]
pub trait SectionHeader: Debug + Pod {
    type Word: Into<u64>;
    type HalfWord: Into<u32>;
    type Xcoff: FileHeader<SectionHeader = Self, Word = Self::Word>;

    fn s_name(&self) -> &[u8; 8];
    fn s_paddr(&self) -> Self::Word;
    fn s_vaddr(&self) -> Self::Word;
    fn s_size(&self) -> Self::Word;
    fn s_scnptr(&self) -> Self::Word;
    fn s_relptr(&self) -> Self::Word;
    fn s_lnnoptr(&self) -> Self::Word;
    fn s_nreloc(&self) -> Self::HalfWord;
    fn s_nlnno(&self) -> Self::HalfWord;
    fn s_flags(&self) -> u32;

    /// Return the section name.
    fn name(&self) -> &[u8] {
        let sectname = &self.s_name()[..];
        match memchr::memchr(b'\0', sectname) {
            Some(end) => &sectname[..end],
            None => sectname,
        }
    }

    /// Return the offset and size of the section in the file.
    fn file_range(&self) -> Option<(u64, u64)> {
        Some((self.s_scnptr().into(), self.s_size().into()))
    }

    /// Return the section data.
    ///
    /// Returns `Ok(&[])` if the section has no data.
    /// Returns `Err` for invalid values.
    fn data<'data, R: ReadRef<'data>>(&self, data: R) -> result::Result<&'data [u8], ()> {
        if let Some((offset, size)) = self.file_range() {
            data.read_bytes_at(offset.into(), size.into())
        } else {
            Ok(&[])
        }
    }
}

impl SectionHeader for xcoff::SectionHeader32 {
    type Word = u32;
    type HalfWord = u16;
    type Xcoff = xcoff::FileHeader32;

    fn s_name(&self) -> &[u8; 8] {
        &self.s_name
    }

    fn s_paddr(&self) -> Self::Word {
        self.s_paddr.get(BE)
    }

    fn s_vaddr(&self) -> Self::Word {
        self.s_vaddr.get(BE)
    }

    fn s_size(&self) -> Self::Word {
        self.s_size.get(BE)
    }

    fn s_scnptr(&self) -> Self::Word {
        self.s_scnptr.get(BE)
    }

    fn s_relptr(&self) -> Self::Word {
        self.s_relptr.get(BE)
    }

    fn s_lnnoptr(&self) -> Self::Word {
        self.s_lnnoptr.get(BE)
    }

    fn s_nreloc(&self) -> Self::HalfWord {
        self.s_nreloc.get(BE)
    }

    fn s_nlnno(&self) -> Self::HalfWord {
        self.s_nlnno.get(BE)
    }

    fn s_flags(&self) -> u32 {
        self.s_flags.get(BE)
    }
}

impl SectionHeader for xcoff::SectionHeader64 {
    type Word = u64;
    type HalfWord = u32;
    type Xcoff = xcoff::FileHeader64;

    fn s_name(&self) -> &[u8; 8] {
        &self.s_name
    }

    fn s_paddr(&self) -> Self::Word {
        self.s_paddr.get(BE)
    }

    fn s_vaddr(&self) -> Self::Word {
        self.s_vaddr.get(BE)
    }

    fn s_size(&self) -> Self::Word {
        self.s_size.get(BE)
    }

    fn s_scnptr(&self) -> Self::Word {
        self.s_scnptr.get(BE)
    }

    fn s_relptr(&self) -> Self::Word {
        self.s_relptr.get(BE)
    }

    fn s_lnnoptr(&self) -> Self::Word {
        self.s_lnnoptr.get(BE)
    }

    fn s_nreloc(&self) -> Self::HalfWord {
        self.s_nreloc.get(BE)
    }

    fn s_nlnno(&self) -> Self::HalfWord {
        self.s_nlnno.get(BE)
    }

    fn s_flags(&self) -> u32 {
        self.s_flags.get(BE)
    }
}
