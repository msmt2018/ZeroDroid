#![forbid(unsafe_code)]

pub mod avb;\npub mod bootconfig;
pub mod arm64_kernel;
pub mod legacy_boot;
pub mod fdt;
pub mod partition_catalog;

pub mod vendor_boot;
pub mod gpt;
pub mod lp_metadata;
use core::ops::Range;
const SPARSE_MAGIC:u32=0xed26_ff3a;
const FILE_HEADER_SIZE:usize=28;
const CHUNK_HEADER_SIZE:usize=12;
#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum ParseError{OutOfBounds,IntegerOverflow,ResourceLimit,InvalidAlignment,InvalidMagic,UnsupportedVersion,InvalidHeaderSize,InvalidBlockSize,InvalidChunkType,InvalidChunkLength,OutputSizeMismatch,ChecksumMismatch,InvalidBootHeader,UnsupportedBootHeaderVersion}
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
const fn make_crc32_table()->[u32;256]{
 let mut table=[0_u32;256];let mut i=0;
 while i<256{let mut value=i as u32;let mut bit=0;while bit<8{value=if value&1!=0{(value>>1)^0xedb8_8320}else{value>>1};bit+=1;}table[i]=value;i+=1;}
 table
}
const CRC32_TABLE:[u32;256]=make_crc32_table();
fn crc32_update(mut crc:u32, bytes:&[u8])->u32{
 for &byte in bytes{let index=((crc^u32::from(byte))&0xff) as usize;crc=(crc>>8)^CRC32_TABLE[index];}
 crc
}
fn gf2_matrix_times(matrix:&[u32;32],mut vector:u32)->u32{
 let mut sum=0_u32;
 let mut index=0_usize;
 while vector!=0{
  if vector&1!=0{sum^=matrix[index];}
  vector>>=1;
  index+=1;
 }
 sum
}
fn gf2_matrix_square(square:&mut [u32;32],matrix:&[u32;32]){
 for n in 0..32{square[n]=gf2_matrix_times(matrix,matrix[n]);}
}
/// Combines two finalized IEEE CRC-32 values without reading either byte stream again.
fn crc32_combine(mut crc1:u32,crc2:u32,mut second_len:u64)->u32{
 if second_len==0{return crc1}
 let mut odd=[0_u32;32];
 let mut even=[0_u32;32];
 odd[0]=0xedb8_8320;
 let mut row=1_u32;
 for slot in odd.iter_mut().skip(1){*slot=row;row<<=1;}
 gf2_matrix_square(&mut even,&odd);
 gf2_matrix_square(&mut odd,&even);
 loop{
  gf2_matrix_square(&mut even,&odd);
  if second_len&1!=0{crc1=gf2_matrix_times(&even,crc1);}
  second_len>>=1;
  if second_len==0{break}
  gf2_matrix_square(&mut odd,&even);
  if second_len&1!=0{crc1=gf2_matrix_times(&odd,crc1);}
  second_len>>=1;
  if second_len==0{break}
 }
 crc1^crc2
}
/// Applies a repeated pattern in O(log(count)) CRC-combine operations and constant memory.
fn crc32_repeat(mut crc_state:u32,pattern:&[u8],count:u64)->u32{
 if pattern.is_empty()||count==0{return crc_state}
 let pattern_len=pattern.len() as u64;
 let repetitions=count/pattern_len;
 let remainder=(count%pattern_len) as usize;
 if repetitions!=0{
  let mut repeated_crc=0_u32;
  let mut block_crc=!crc32_update(!0_u32,pattern);
  let mut block_len=pattern_len;
  let mut remaining=repetitions;
  while remaining!=0{
   if remaining&1!=0{
    repeated_crc=crc32_combine(repeated_crc,block_crc,block_len);
   }
   remaining>>=1;
   if remaining!=0{
    block_crc=crc32_combine(block_crc,block_crc,block_len);
    block_len=block_len.saturating_mul(2);
   }
  }
  let combined=crc32_combine(!crc_state,repeated_crc,repetitions.saturating_mul(pattern_len));
  crc_state=!combined;
 }
 if remainder!=0{
  crc_state=crc32_update(crc_state,&pattern[..remainder]);
 }
 crc_state
}

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
 let minimum_chunk_bytes=u64::from(h.total_chunks).checked_mul(u64::from(h.chunk_header_size)).ok_or(ParseError::IntegerOverflow)?;
 let remaining=u64::try_from(data.len()-cursor).map_err(|_|ParseError::IntegerOverflow)?;
 if minimum_chunk_bytes>remaining{return Err(ParseError::OutOfBounds)}
 let mut output_blocks=0_u64;
 let mut crc_state=!0_u32;
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
   0xcac1=>{let n=u64::from(blocks).checked_mul(u64::from(h.block_size)).ok_or(ParseError::IntegerOverflow)?;let n=usize::try_from(n).map_err(|_|ParseError::IntegerOverflow)?;if total!=usize::from(h.chunk_header_size).checked_add(n).ok_or(ParseError::IntegerOverflow)?{return Err(ParseError::InvalidChunkLength)}crc_state=crc32_update(crc_state,&data[payload_start..end]);(SparseChunkKind::Raw,Some(payload_start..end),true)}
   0xcac2=>{if total!=usize::from(h.chunk_header_size)+4{return Err(ParseError::InvalidChunkLength)}crc_state=crc32_repeat(crc_state,&data[payload_start..end],u64::from(blocks)*u64::from(h.block_size));(SparseChunkKind::Fill,Some(payload_start..end),true)}
   0xcac3=>{if total!=usize::from(h.chunk_header_size){return Err(ParseError::InvalidChunkLength)}crc_state=crc32_repeat(crc_state,&[0],u64::from(blocks)*u64::from(h.block_size));(SparseChunkKind::DontCare,None,true)}
   0xcac4=>{if blocks!=0||total!=usize::from(h.chunk_header_size)+4{return Err(ParseError::InvalidChunkLength)}if u32le(data,payload_start)?!=!crc_state{return Err(ParseError::ChecksumMismatch)}(SparseChunkKind::Crc32,Some(payload_start..end),false)}
   _=>return Err(ParseError::InvalidChunkType)
  };
  let offset=output_blocks;
  if contributes{output_blocks=output_blocks.checked_add(u64::from(blocks)).ok_or(ParseError::IntegerOverflow)?;if output_blocks>u64::from(h.total_blocks){return Err(ParseError::OutputSizeMismatch)}}
  chunks.push(SparseChunk{kind:k,output_block_offset:offset,output_block_count:if contributes{blocks}else{0},payload});
  cursor=end;
 }
 if output_blocks!=u64::from(h.total_blocks)||cursor!=data.len(){return Err(ParseError::OutputSizeMismatch)}
 if h.image_checksum!=0&&h.image_checksum!=!crc_state{return Err(ParseError::ChecksumMismatch)}
 Ok(SparseImageIndex{header:h,expanded_size_bytes:expanded,chunks})
}

const ANDROID_BOOT_MAGIC:&[u8;8]=b"ANDROID!";
const BOOT_V3_HEADER_SIZE:usize=1580;
const BOOT_V4_HEADER_SIZE:usize=1584;
const BOOT_PAGE_SIZE:u64=4096;

#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub struct AndroidBootHeaderV3V4{
 pub header_version:u32,
 pub header_size:u32,
 pub kernel_size:u32,
 pub ramdisk_size:u32,
 pub os_version:u32,
 pub signature_size:u32,
}
#[derive(Debug,Clone,PartialEq,Eq)]
pub struct AndroidBootImageIndex{
 pub header:AndroidBootHeaderV3V4,
 pub header_page:Range<usize>,
 pub kernel:Range<usize>,
 pub ramdisk:Range<usize>,
 pub boot_signature:Option<Range<usize>>,
 pub total_indexed_bytes:u64,
}
/// Indexes Android boot image v3/v4 payloads. Payloads remain borrowed ranges into input.
pub fn parse_android_boot_image_v3_v4(data:&[u8],budget:ParseBudget,max_payload_bytes:u64)->Result<AndroidBootImageIndex,ParseError>{
 budget.check_input(u64::try_from(data.len()).map_err(|_|ParseError::IntegerOverflow)?)?;
 if data.len()<BOOT_V3_HEADER_SIZE{return Err(ParseError::OutOfBounds)}
 if data.get(..8)!=Some(ANDROID_BOOT_MAGIC.as_slice()){return Err(ParseError::InvalidMagic)}
 let kernel_size=u32le(data,8)?;
 let ramdisk_size=u32le(data,12)?;
 let os_version=u32le(data,16)?;
 let header_size=u32le(data,20)?;
 if data.get(24..40).ok_or(ParseError::OutOfBounds)?.iter().any(|b|*b!=0){return Err(ParseError::InvalidBootHeader)}
 let header_version=u32le(data,40)?;
 let expected_header_size=match header_version{3=>BOOT_V3_HEADER_SIZE,4=>BOOT_V4_HEADER_SIZE,_=>return Err(ParseError::UnsupportedBootHeaderVersion)};
 if kernel_size==0{return Err(ParseError::InvalidBootHeader)}
 if usize::try_from(header_size).map_err(|_|ParseError::IntegerOverflow)?!=expected_header_size{return Err(ParseError::InvalidBootHeader)}
 if data.len()<expected_header_size{return Err(ParseError::OutOfBounds)}
 let signature_size=if header_version==4{u32le(data,1580)?}else{0};
 let payload_total=u64::from(kernel_size).checked_add(u64::from(ramdisk_size)).and_then(|n|n.checked_add(u64::from(signature_size))).ok_or(ParseError::IntegerOverflow)?;
 if payload_total>max_payload_bytes{return Err(ParseError::ResourceLimit)}
 let kernel_start=BOOT_PAGE_SIZE;
 let kernel_end=kernel_start.checked_add(u64::from(kernel_size)).ok_or(ParseError::IntegerOverflow)?;
 let ramdisk_start=checked_align_up(kernel_end,BOOT_PAGE_SIZE)?;
 let ramdisk_end=ramdisk_start.checked_add(u64::from(ramdisk_size)).ok_or(ParseError::IntegerOverflow)?;
 let signature_start=checked_align_up(ramdisk_end,BOOT_PAGE_SIZE)?;
 let signature_end=signature_start.checked_add(u64::from(signature_size)).ok_or(ParseError::IntegerOverflow)?;
 let indexed_end=if header_version==4{signature_end}else{ramdisk_end};
 if indexed_end>u64::try_from(data.len()).map_err(|_|ParseError::IntegerOverflow)?{return Err(ParseError::OutOfBounds)}
 let to_range=|start:u64,end:u64|->Result<Range<usize>,ParseError>{Ok(usize::try_from(start).map_err(|_|ParseError::IntegerOverflow)?..usize::try_from(end).map_err(|_|ParseError::IntegerOverflow)?)};
 let header=AndroidBootHeaderV3V4{header_version,header_size,kernel_size,ramdisk_size,os_version,signature_size};
 Ok(AndroidBootImageIndex{
  header,
  header_page:to_range(0,BOOT_PAGE_SIZE)?,
  kernel:to_range(kernel_start,kernel_end)?,
  ramdisk:to_range(ramdisk_start,ramdisk_end)?,
  boot_signature:if header_version==4{Some(to_range(signature_start,signature_end)?)}else{None},
  total_indexed_bytes:indexed_end,
 })
}


#[derive(Debug,Clone,PartialEq,Eq)]
pub struct InitBootImageIndex{
 pub header:AndroidBootHeaderV3V4,
 pub header_page:Range<usize>,
 pub ramdisk:Range<usize>,
 pub boot_signature:Option<Range<usize>>,
 pub total_indexed_bytes:u64,
}
/// Parses the standalone init_boot v4 image. Unlike boot.img, its kernel payload must be empty.
pub fn parse_init_boot_image_v4(data:&[u8],budget:ParseBudget,max_payload_bytes:u64)->Result<InitBootImageIndex,ParseError>{
 budget.check_input(u64::try_from(data.len()).map_err(|_|ParseError::IntegerOverflow)?)?;
 if data.len()<BOOT_V4_HEADER_SIZE{return Err(ParseError::OutOfBounds)}
 if data.get(..8)!=Some(ANDROID_BOOT_MAGIC.as_slice()){return Err(ParseError::InvalidMagic)}
 let kernel_size=u32le(data,8)?;
 let ramdisk_size=u32le(data,12)?;
 let os_version=u32le(data,16)?;
 let header_size=u32le(data,20)?;
 if data.get(24..40).ok_or(ParseError::OutOfBounds)?.iter().any(|b|*b!=0){return Err(ParseError::InvalidBootHeader)}
 let header_version=u32le(data,40)?;
 if header_version!=4{return Err(ParseError::UnsupportedBootHeaderVersion)}
 if kernel_size!=0||ramdisk_size==0||header_size as usize!=BOOT_V4_HEADER_SIZE{return Err(ParseError::InvalidBootHeader)}
 let signature_size=u32le(data,1580)?;
 let payload_total=u64::from(ramdisk_size).checked_add(u64::from(signature_size)).ok_or(ParseError::IntegerOverflow)?;
 if payload_total>max_payload_bytes{return Err(ParseError::ResourceLimit)}
 let ramdisk_start=BOOT_PAGE_SIZE;
 let ramdisk_end=ramdisk_start.checked_add(u64::from(ramdisk_size)).ok_or(ParseError::IntegerOverflow)?;
 let signature_start=checked_align_up(ramdisk_end,BOOT_PAGE_SIZE)?;
 let signature_end=signature_start.checked_add(u64::from(signature_size)).ok_or(ParseError::IntegerOverflow)?;
 let indexed_end=if signature_size==0{ramdisk_end}else{signature_end};
 if indexed_end>u64::try_from(data.len()).map_err(|_|ParseError::IntegerOverflow)?{return Err(ParseError::OutOfBounds)}
 let to_range=|start:u64,end:u64|->Result<Range<usize>,ParseError>{Ok(usize::try_from(start).map_err(|_|ParseError::IntegerOverflow)?..usize::try_from(end).map_err(|_|ParseError::IntegerOverflow)?)};
 Ok(InitBootImageIndex{
  header:AndroidBootHeaderV3V4{header_version,header_size,kernel_size,ramdisk_size,os_version,signature_size},
  header_page:to_range(0,BOOT_PAGE_SIZE)?,
  ramdisk:to_range(ramdisk_start,ramdisk_end)?,
  boot_signature:if signature_size==0{None}else{Some(to_range(signature_start,signature_end)?)},
  total_indexed_bytes:indexed_end,
 })
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
 #[test]fn validates_crc32_chunk_and_header_checksum(){
  let raw=[0x5a_u8;4096];let crc=!crc32_update(!0_u32,&raw);
  let mut v=header(1,2);v.extend_from_slice(&0xcac1_u16.to_le_bytes());v.extend_from_slice(&0_u16.to_le_bytes());v.extend_from_slice(&1_u32.to_le_bytes());v.extend_from_slice(&4108_u32.to_le_bytes());v.extend_from_slice(&raw);
  v.extend_from_slice(&0xcac4_u16.to_le_bytes());v.extend_from_slice(&0_u16.to_le_bytes());v.extend_from_slice(&0_u32.to_le_bytes());v.extend_from_slice(&16_u32.to_le_bytes());v.extend_from_slice(&crc.to_le_bytes());
  v[24..28].copy_from_slice(&crc.to_le_bytes());
  assert!(parse_android_sparse_image(&v,budget(),4096).is_ok());
  let last=v.len()-1;v[last]^=1;assert_eq!(parse_android_sparse_image(&v,budget(),4096),Err(ParseError::ChecksumMismatch));
 }
 #[test]fn validates_crc_for_fill_and_dont_care(){
  let mut v=header(2,2);v.extend_from_slice(&0xcac2_u16.to_le_bytes());v.extend_from_slice(&0_u16.to_le_bytes());v.extend_from_slice(&1_u32.to_le_bytes());v.extend_from_slice(&16_u32.to_le_bytes());v.extend_from_slice(&[1,2,3,4]);
  v.extend_from_slice(&0xcac3_u16.to_le_bytes());v.extend_from_slice(&0_u16.to_le_bytes());v.extend_from_slice(&1_u32.to_le_bytes());v.extend_from_slice(&12_u32.to_le_bytes());
  let mut expanded=[0_u8;8192];for i in 0..1024{expanded[i*4..i*4+4].copy_from_slice(&[1,2,3,4]);}
  let expected=!crc32_update(!0_u32,&expanded);v[24..28].copy_from_slice(&expected.to_le_bytes());
  assert!(parse_android_sparse_image(&v,budget(),8192).is_ok());
 }

 #[test]fn crc32_repeat_matches_bytewise_reference(){
  for pattern in [&[0_u8][..],&[1_u8,2,3,4][..],&[0xa5_u8,0x5a,0x11][..]]{
   for count in 0_u64..257{
    let mut expected=!0_u32;
    for index in 0..count{let offset=(index as usize)%pattern.len();expected=crc32_update(expected,&pattern[offset..offset+1]);}
    assert_eq!(crc32_repeat(!0_u32,pattern,count),expected,"pattern={pattern:?}, count={count}");
   }
  }
 }
 #[test]fn crc32_repeat_handles_multi_gigabyte_sparse_runs_in_logarithmic_work(){
  assert_eq!(crc32_repeat(!0_u32,&[0],4_u64*1024*1024*1024),!0xd202_ef8d_u32);
  assert_eq!(crc32_repeat(!0_u32,&[0x12,0x34,0x56,0x78],4_u64*1024*1024*1024),!0xe369_2abd_u32);
 }
 #[test]fn boot_v3_indexes_kernel_and_ramdisk_without_copying(){
  let mut v=vec![0_u8;8192+4096];v[..8].copy_from_slice(b"ANDROID!");v[8..12].copy_from_slice(&4096_u32.to_le_bytes());v[12..16].copy_from_slice(&1024_u32.to_le_bytes());v[20..24].copy_from_slice(&1580_u32.to_le_bytes());v[40..44].copy_from_slice(&3_u32.to_le_bytes());
  let p=parse_android_boot_image_v3_v4(&v,budget(),8192).unwrap();assert_eq!(p.kernel,4096..8192);assert_eq!(p.ramdisk,8192..9216);assert_eq!(p.boot_signature,None);
 }
 #[test]fn boot_v4_indexes_signature_and_checks_budget(){
  let mut v=vec![0_u8;12288+512];v[..8].copy_from_slice(b"ANDROID!");v[8..12].copy_from_slice(&4096_u32.to_le_bytes());v[12..16].copy_from_slice(&1024_u32.to_le_bytes());v[20..24].copy_from_slice(&1584_u32.to_le_bytes());v[40..44].copy_from_slice(&4_u32.to_le_bytes());v[1580..1584].copy_from_slice(&512_u32.to_le_bytes());
  let p=parse_android_boot_image_v3_v4(&v,budget(),8192).unwrap();assert_eq!(p.boot_signature,Some(12288..12800));assert_eq!(p.header.signature_size,512);
  assert_eq!(parse_android_boot_image_v3_v4(&v,budget(),5631),Err(ParseError::ResourceLimit));
 }
 #[test]fn boot_parser_rejects_version_header_and_truncation(){
  let mut v=vec![0_u8;8192];v[..8].copy_from_slice(b"ANDROID!");v[20..24].copy_from_slice(&1580_u32.to_le_bytes());v[40..44].copy_from_slice(&5_u32.to_le_bytes());assert_eq!(parse_android_boot_image_v3_v4(&v,budget(),1),Err(ParseError::UnsupportedBootHeaderVersion));
  v[40..44].copy_from_slice(&3_u32.to_le_bytes());v[20..24].copy_from_slice(&1_u32.to_le_bytes());assert_eq!(parse_android_boot_image_v3_v4(&v,budget(),1),Err(ParseError::InvalidBootHeader));
  v[20..24].copy_from_slice(&1580_u32.to_le_bytes());v[8..12].copy_from_slice(&8192_u32.to_le_bytes());v[12..16].copy_from_slice(&1024_u32.to_le_bytes());assert_eq!(parse_android_boot_image_v3_v4(&v,budget(),1),Err(ParseError::ResourceLimit));assert_eq!(parse_android_boot_image_v3_v4(&v,budget(),u64::MAX),Err(ParseError::OutOfBounds));
 }
 #[test]fn init_boot_v4_indexes_ramdisk_without_kernel(){
  let mut v=vec![0_u8;8192];v[..8].copy_from_slice(b"ANDROID!");v[12..16].copy_from_slice(&4096_u32.to_le_bytes());v[20..24].copy_from_slice(&1584_u32.to_le_bytes());v[40..44].copy_from_slice(&4_u32.to_le_bytes());
  let p=parse_init_boot_image_v4(&v,budget(),4096).unwrap();assert_eq!(p.header.kernel_size,0);assert_eq!(p.ramdisk,4096..8192);assert_eq!(p.boot_signature,None);
 }
 #[test]fn init_boot_rejects_kernel_wrong_version_and_truncation(){
  let mut v=vec![0_u8;8192];v[..8].copy_from_slice(b"ANDROID!");v[12..16].copy_from_slice(&4096_u32.to_le_bytes());v[20..24].copy_from_slice(&1584_u32.to_le_bytes());v[40..44].copy_from_slice(&4_u32.to_le_bytes());
  v[8..12].copy_from_slice(&1_u32.to_le_bytes());assert_eq!(parse_init_boot_image_v4(&v,budget(),4096),Err(ParseError::InvalidBootHeader));
  v[8..12].fill(0);v[40..44].copy_from_slice(&3_u32.to_le_bytes());assert_eq!(parse_init_boot_image_v4(&v,budget(),4096),Err(ParseError::UnsupportedBootHeaderVersion));
  v[40..44].copy_from_slice(&4_u32.to_le_bytes());assert_eq!(parse_init_boot_image_v4(&v[..4096],budget(),4096),Err(ParseError::OutOfBounds));
 }
 #[test]fn init_boot_enforces_ramdisk_budget_and_signature_range(){
  let mut v=vec![0_u8;12288+256];v[..8].copy_from_slice(b"ANDROID!");v[12..16].copy_from_slice(&4096_u32.to_le_bytes());v[20..24].copy_from_slice(&1584_u32.to_le_bytes());v[40..44].copy_from_slice(&4_u32.to_le_bytes());v[1580..1584].copy_from_slice(&256_u32.to_le_bytes());
  let p=parse_init_boot_image_v4(&v,budget(),4352).unwrap();assert_eq!(p.boot_signature,Some(8192..8448));
  assert_eq!(parse_init_boot_image_v4(&v,budget(),4351),Err(ParseError::ResourceLimit));
 }

 #[test]fn rejects_output_block_mismatch(){let mut v=header(2,1);v.extend_from_slice(&0xcac3_u16.to_le_bytes());v.extend_from_slice(&0_u16.to_le_bytes());v.extend_from_slice(&1_u32.to_le_bytes());v.extend_from_slice(&12_u32.to_le_bytes());assert_eq!(parse_android_sparse_image(&v,budget(),8192),Err(ParseError::OutputSizeMismatch));}
}
