#![forbid(unsafe_code)]
use core::ops::Range;
const SPARSE_MAGIC:u32=0xed26_ff3a;
const FILE_HEADER_SIZE:usize=28;
const CHUNK_HEADER_SIZE:usize=12;
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum ParseError{OutOfBounds,IntegerOverflow,ResourceLimit,InvalidAlignment,InvalidMagic,UnsupportedVersion,InvalidHeaderSize,InvalidBlockSize,InvalidChunkType,InvalidChunkLength,OutputSizeMismatch}
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub struct ParseBudget{pub max_input_bytes:u64,pub max_items:u32,pub max_nesting_depth:u16}
impl ParseBudget{
 pub fn check_input(&self,n:u64)->Result<(),ParseError>{if n>self.max_input_bytes{Err(ParseError::ResourceLimit)}else{Ok(())}}
 pub fn check_items(&self,n:u32)->Result<(),ParseError>{if n>self.max_items{Err(ParseError::ResourceLimit)}else{Ok(())}}
}
pub fn checked_range(offset:u64,length:u64,total:u64)->Result<Range<usize>,ParseError>{
 let end=offset.checked_add(length).ok_or(ParseError::IntegerOverflow)?;
 if end>total{return Err(ParseError::OutOfBounds)}
 Ok(usize::try_from(offset).map_err(|_|ParseError::OutOfBounds)?..usize::try_from(end).map_err(|_|ParseError::OutOfBounds)?)
}
pub fn checked_align_up(value:u64,alignment:u64)->Result<u64,ParseError>{
 if alignment==0||!alignment.is_power_of_two(){return Err(ParseError::InvalidAlignment)}
 value.checked_add(alignment-1).map(|v|v&!(alignment-1)).ok_or(ParseError::IntegerOverflow)
}
fn u16le(d:&[u8],o:usize)->Result<u16,ParseError>{let b=d.get(o..o.checked_add(2).ok_or(ParseError::IntegerOverflow)?).ok_or(ParseError::OutOfBounds)?;Ok(u16::from_le_bytes([b[0],b[1]]))}
fn u32le(d:&[u8],o:usize)->Result<u32,ParseError>{let b=d.get(o..o.checked_add(4).ok_or(ParseError::IntegerOverflow)?).ok_or(ParseError::OutOfBounds)?;Ok(u32::from_le_bytes([b[0],b[1],b[2],b[3]]))}
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub struct SparseImageHeader{pub major_version:u16,pub minor_version:u16,pub file_header_size:u16,pub chunk_header_size:u16,pub block_size:u32,pub total_blocks:u32,pub total_chunks:u32,pub image_checksum:u32}
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum SparseChunkKind{Raw,Fill,DontCare,Crc32}
#[derive(Debug,Clone,PartialEq,Eq)]
pub struct SparseChunk{pub kind:SparseChunkKind,pub output_block_offset:u64,pub output_block_count:u32,pub payload:Option<Range<usize>>}
#[derive(Debug,Clone,PartialEq,Eq)]
pub struct SparseImageIndex{pub header:SparseImageHeader,pub expanded_size_bytes:u64,pub chunks:Vec<SparseChunk>}
/// Validates Android sparse v1 framing and indexes payload ranges without expanding or copying data.
pub fn parse_android_sparse_image(data:&[u8],budget:ParseBudget,max_expanded_bytes:u64)->Result<SparseImageIndex,ParseError>{
 budget.check_input(u64::try_from(data.len()).map_err(|_|ParseError::IntegerOverflow)?)?;
 if data.len()<FILE_HEADER_SIZE{return Err(ParseError::OutOfBounds)}
 if u32le(data,0)?!=SPARSE_MAGIC{return Err(ParseError::InvalidMagic)}
 let h=SparseImageHeader{major_version:u16le(data,4)?,minor_version:u16le(data,6)?,file_header_size:u16le(data,8)?,chunk_header_size:u16le(data,10)?,block_size:u32le(data,12)?,total_blocks:u32le(data,16)?,total_chunks:u32le(data,20)?,image_checksum:u32le(data,24)?};
 if h.major_version!=1{return Err(ParseError::UnsupportedVersion)}
 if usize::from(h.file_header_size)<FILE_HEADER_SIZE||usize::from(h.chunk_header_size)<CHUNK_HEADER_SIZE{return Err(ParseError::InvalidHeaderSize)}
 if h.block_size==0||h.block_size%4!=0{return Err(ParseError::InvalidBlockSize)}
 budget.check_items(h.total_chunks)?;
 let expanded=u64::from(h.total_blocks).checked_mul(u64::from(h.block_size)).ok_or(ParseError::IntegerOverflow)?;
 if expanded>max_expanded_bytes{return Err(ParseError::ResourceLimit)}
 let mut cursor=usize::from(h.file_header_size);
 if cursor>data.len(){return Err(ParseError::OutOfBounds)}
 let mut output_blocks=0_u64;
 let mut chunks=Vec::with_capacity(h.total_chunks as usize);
 for _ in 0..h.total_chunks{
  let header_end=cursor.checked_add(usize::from(h.chunk_header_size)).ok_or(ParseError::IntegerOverflow)?;
  let ch=data.get(cursor..header_end).ok_or(ParseError::OutOfBounds)?;
  let kind=u16le(ch,0)?;let reserved=u16le(ch,2)?;let blocks=u32le(ch,4)?;
  let total=usize::try_from(u32le(ch,8)?).map_err(|_|ParseError::IntegerOverflow)?;
  if reserved!=0||total<usize::from(h.chunk_header_size){return Err(ParseError::InvalidChunkLength)}
  let payload_start=header_end;let end=cursor.checked_add(total).ok_or(ParseError::IntegerOverflow)?;
  if end>data.len(){return Err(ParseError::OutOfBounds)}
  let(k,payload,contributes)=match kind{
   0xcac1=>{let n=u64::from(blocks).checked_mul(u64::from(h.block_size)).ok_or(ParseError::IntegerOverflow)?;let n=usize::try_from(n).map_err(|_|ParseError::IntegerOverflow)?;if total!=usize::from(h.chunk_header_size).checked_add(n).ok_or(ParseError::IntegerOverflow)?{return Err(ParseError::InvalidChunkLength)}(SparseChunkKind::Raw,Some(payload_start..end),true)}
   0xcac2=>{if total!=usize::from(h.chunk_header_size)+4{return Err(ParseError::InvalidChunkLength)}(SparseChunkKind::Fill,Some(payload_start..end),true)}
   0xcac3=>{if total!=usize::from(h.chunk_header_size){return Err(ParseError::InvalidChunkLength)}(SparseChunkKind::DontCare,None,true)}
   0xcac4=>{if blocks!=0||total!=usize::from(h.chunk_header_size)+4{return Err(ParseError::InvalidChunkLength)}(SparseChunkKind::Crc32,Some(payload_start..end),false)}
   _=>return Err(ParseError::InvalidChunkType)
  };
  let offset=output_blocks;
  if contributes{output_blocks=output_blocks.checked_add(u64::from(blocks)).ok_or(ParseError::IntegerOverflow)?;if output_blocks>u64::from(h.total_blocks){return Err(ParseError::OutputSizeMismatch)}}
  chunks.push(SparseChunk{kind:k,output_block_offset:offset,output_block_count:if contributes{blocks}else{0},payload});
  cursor=end;
 }
 if output_blocks!=u64::from(h.total_blocks)||cursor!=data.len(){return Err(ParseError::OutputSizeMismatch)}
 Ok(SparseImageIndex{header:h,expanded_size_bytes:expanded,chunks})
}
#[cfg(test)]
mod tests{
 use super::*;
 fn header(blocks:u32,chunks:u32)->Vec<u8>{let mut v=Vec::new();v.extend_from_slice(&SPARSE_MAGIC.to_le_bytes());v.extend_from_slice(&1_u16.to_le_bytes());v.extend_from_slice(&0_u16.to_le_bytes());v.extend_from_slice(&28_u16.to_le_bytes());v.extend_from_slice(&12_u16.to_le_bytes());v.extend_from_slice(&4096_u32.to_le_bytes());v.extend_from_slice(&blocks.to_le_bytes());v.extend_from_slice(&chunks.to_le_bytes());v.extend_from_slice(&0_u32.to_le_bytes());v}
 fn budget()->ParseBudget{ParseBudget{max_input_bytes:1<<20,max_items:128,max_nesting_depth:8}}
 #[test]fn bounded_range(){assert_eq!(checked_range(4,8,12),Ok(4..12));}
 #[test]fn overflow(){assert_eq!(checked_range(u64::MAX,2,u64::MAX),Err(ParseError::IntegerOverflow));}
 #[test]fn out_of_bounds(){assert_eq!(checked_range(8,8,12),Err(ParseError::OutOfBounds));}
 #[test]fn alignment(){assert_eq!(checked_align_up(9,8),Ok(16));assert_eq!(checked_align_up(9,3),Err(ParseError::InvalidAlignment));}
 #[test]fn budget_limit(){assert_eq!(budget().check_input((1<<20)+1),Err(ParseError::ResourceLimit));}
 #[test]fn raw_chunk_is_indexed_without_expansion(){let mut v=header(1,1);v.extend_from_slice(&0xcac1_u16.to_le_bytes());v.extend_from_slice(&0_u16.to_le_bytes());v.extend_from_slice(&1_u32.to_le_bytes());v.extend_from_slice(&4108_u32.to_le_bytes());v.extend(std::iter::repeat(0x5a).take(4096));let p=parse_android_sparse_image(&v,budget(),4096).unwrap();assert_eq!(p.expanded_size_bytes,4096);assert_eq!(p.chunks[0].payload,Some(40..4136));}
 #[test]fn fill_and_dont_care_chunks(){let mut v=header(3,2);v.extend_from_slice(&0xcac2_u16.to_le_bytes());v.extend_from_slice(&0_u16.to_le_bytes());v.extend_from_slice(&2_u32.to_le_bytes());v.extend_from_slice(&16_u32.to_le_bytes());v.extend_from_slice(&[1,2,3,4]);v.extend_from_slice(&0xcac3_u16.to_le_bytes());v.extend_from_slice(&0_u16.to_le_bytes());v.extend_from_slice(&1_u32.to_le_bytes());v.extend_from_slice(&12_u32.to_le_bytes());let p=parse_android_sparse_image(&v,budget(),12288).unwrap();assert_eq!(p.chunks[0].kind,SparseChunkKind::Fill);assert_eq!(p.chunks[1].kind,SparseChunkKind::DontCare);assert_eq!(p.chunks[1].output_block_offset,2);}
 #[test]fn rejects_bad_magic_and_truncation(){let mut v=header(0,0);v[0]=0;assert_eq!(parse_android_sparse_image(&v,budget(),0),Err(ParseError::InvalidMagic));let mut v=header(1,1);v.extend_from_slice(&[0xc1,0xca,0,0,1,0,0,0,0xff,0xff,0,0]);assert_eq!(parse_android_sparse_image(&v,budget(),4096),Err(ParseError::OutOfBounds));}
 #[test]fn enforces_output_and_item_budgets(){let mut v=header(1,0);assert_eq!(parse_android_sparse_image(&v,budget(),4095),Err(ParseError::ResourceLimit));v[20..24].copy_from_slice(&129_u32.to_le_bytes());assert_eq!(parse_android_sparse_image(&v,budget(),4096),Err(ParseError::ResourceLimit));}
 #[test]fn rejects_output_block_mismatch(){let mut v=header(2,1);v.extend_from_slice(&0xcac3_u16.to_le_bytes());v.extend_from_slice(&0_u16.to_le_bytes());v.extend_from_slice(&1_u32.to_le_bytes());v.extend_from_slice(&12_u32.to_le_bytes());assert_eq!(parse_android_sparse_image(&v,budget(),8192),Err(ParseError::OutputSizeMismatch));}
}
