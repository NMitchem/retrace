// cacheprobe.c — empirically dump the dyld shared cache fixup/slide format on THIS host.
//
// For the main cache + each arm64e subcache file it: parses dyld_cache_header
// (mappingOffset/Count, mappingWithSlideOffset/Count), prints the plain mappings and the
// mapping_and_slide entries (address/size/fileOffset/slideInfoFileOffset+Size/prot), reads
// the slide-info VERSION, and for v3/v5 dumps the slide struct + walks the first data page's
// chained fixups, decoding a few auth/rebase slots BY HAND to verify the bit layout.
//
// Pure host file parsing (pread into heap) — no HVF, no mmap of cache pages. Safe to run.
//   clang -O2 -o cacheprobe cacheprobe.c && ./cacheprobe
#include <stdio.h>
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <inttypes.h>

// ---- structures (dyld_cache_format.h layout; verified via offsetof + raw bytes below) ----
struct dyld_cache_header {                 // only the prefix we need
    char     magic[16];                    // 0x00
    uint32_t mappingOffset;                // 0x10
    uint32_t mappingCount;                 // 0x14
    uint32_t imagesOffsetOld;              // 0x18
    uint32_t imagesCountOld;               // 0x1C
    uint64_t dyldBaseAddress;              // 0x20
    uint64_t codeSignatureOffset;          // 0x28
    uint64_t codeSignatureSize;            // 0x30
    uint64_t slideInfoOffsetUnused;        // 0x38
    uint64_t slideInfoSizeUnused;          // 0x40
    uint64_t localSymbolsOffset;           // 0x48
    uint64_t localSymbolsSize;             // 0x50
    uint8_t  uuid[16];                     // 0x58
    uint64_t cacheType;                    // 0x68
    uint32_t branchPoolsOffset;            // 0x70
    uint32_t branchPoolsCount;             // 0x74
    uint64_t dyldInCacheMH;                // 0x78
    uint64_t dyldInCacheEntry;             // 0x80
    uint64_t imagesTextOffset;             // 0x88
    uint64_t imagesTextCount;              // 0x90
    uint64_t patchInfoAddr;                // 0x98
    uint64_t patchInfoSize;                // 0xA0
    uint64_t otherImageGroupAddrUnused;    // 0xA8
    uint64_t otherImageGroupSizeUnused;    // 0xB0
    uint64_t progClosuresAddr;             // 0xB8
    uint64_t progClosuresSize;             // 0xC0
    uint64_t progClosuresTrieAddr;         // 0xC8
    uint64_t progClosuresTrieSize;         // 0xD0
    uint32_t platform;                     // 0xD8
    uint32_t formatVersion_bits;           // 0xDC
    uint64_t sharedRegionStart;            // 0xE0
    uint64_t sharedRegionSize;             // 0xE8
    uint64_t maxSlide;                     // 0xF0
    uint64_t dylibsImageArrayAddr;         // 0xF8
    uint64_t dylibsImageArraySize;         // 0x100
    uint64_t dylibsTrieAddr;               // 0x108
    uint64_t dylibsTrieSize;               // 0x110
    uint64_t otherImageArrayAddr;          // 0x118
    uint64_t otherImageArraySize;          // 0x120
    uint64_t otherTrieAddr;                // 0x128
    uint64_t otherTrieSize;                // 0x130
    uint32_t mappingWithSlideOffset;       // 0x138
    uint32_t mappingWithSlideCount;        // 0x13C
};

struct dyld_cache_mapping_info {           // 32 bytes
    uint64_t address, size, fileOffset;
    uint32_t maxProt, initProt;
};
struct dyld_cache_mapping_and_slide_info { // 56 bytes
    uint64_t address, size, fileOffset;
    uint64_t slideInfoFileOffset, slideInfoFileSize;
    uint64_t flags;
    uint32_t maxProt, initProt;
};

// slide info v3 (arm64e, macOS 11..) and v5 (arm64e, macOS 14+): same prefix layout.
struct dyld_cache_slide_info_v3_or_v5 {
    uint32_t version;            // 3 or 5
    uint32_t page_size;          // 4096 or 16384
    uint32_t page_starts_count;
    uint64_t value_add;          // v3: auth_value_add
    uint16_t page_starts[];      // [page_starts_count]
};
#define NO_REBASE 0xFFFF

static const char *protstr(uint32_t p){ // VM_PROT_READ=1 WRITE=2 EXEC=4
    static char b[8]; b[0]=(p&1)?'r':'-'; b[1]=(p&2)?'w':'-'; b[2]=(p&4)?'x':'-'; b[3]=0; return b; }

static void *slurp(int fd, uint64_t off, uint64_t len){
    void *p = malloc(len); if(!p) return NULL;
    if (pread(fd, p, len, off) != (ssize_t)len){ free(p); return NULL; } return p; }

// Decode + print one v3 slide pointer slot. `slotVA` is the slot's own VA at slide 0.
static uint32_t decode_v3(uint64_t raw, uint64_t value_add, int idx, uint64_t slotVA, uint32_t off){
    uint32_t authenticated = (raw >> 63) & 1;
    uint32_t next          = (raw >> 51) & 0x7FF;   // offsetToNextPointer (8-byte units)
    if (authenticated){
        uint32_t off32 = raw & 0xFFFFFFFF;          // offsetFromSharedCacheBase
        uint32_t div   = (raw >> 32) & 0xFFFF;      // diversityData
        uint32_t adiv  = (raw >> 48) & 1;           // hasAddressDiversity
        uint32_t key   = (raw >> 49) & 3;           // 0=IA 1=IB 2=DA 3=DB
        const char *kn[4]={"IA","IB","DA","DB"};
        printf("      [%2d] @0x%04x AUTH raw=0x%016" PRIx64 " off=0x%08x div=0x%04x addrDiv=%u key=%s next=%u\n",
               idx, off, raw, off32, div, adiv, kn[key], next);
        printf("           => @slide0: slotVA=0x%" PRIx64 " targetVA = base + 0x%08x (+value_add 0x%" PRIx64 ")\n",
               slotVA, off32, value_add);
    } else {
        uint64_t pv = raw & 0x7FFFFFFFFFFFF;         // pointerValue (51 bits)
        printf("      [%2d] @0x%04x PLAIN raw=0x%016" PRIx64 " pointerValue=0x%013" PRIx64 " next=%u\n",
               idx, off, raw, pv, next);
    }
    return next;
}

// Decode + print one v5 slide pointer slot. `slotVA` is the slot's own VA at slide 0 and `off` its
// byte offset within the page — together with the printed target VA these ARE the three
// worked-example constants `crates/retrace-box/tests/cache_pager.rs` pins (slot IPA, target,
// diversity), so re-deriving them after a cache drift is a matter of reading an AUTH line here.
static uint32_t decode_v5(uint64_t raw, uint64_t value_add, int idx, uint64_t slotVA, uint32_t off){
    uint32_t isAuth = (raw >> 63) & 1;
    uint32_t next   = (raw >> 52) & 0x7FF;          // 8-byte units
    uint64_t roff   = raw & 0x3FFFFFFFFULL;         // runtimeOffset (34 bits)
    if (isAuth){
        uint32_t div  = (raw >> 34) & 0xFFFF;       // diversity
        uint32_t adiv = (raw >> 50) & 1;            // addrDiv
        uint32_t kd   = (raw >> 51) & 1;            // keyIsData: 0=IA 1=DA (A-family only)
        printf("      [%2d] @0x%04x AUTH raw=0x%016" PRIx64 " roff=0x%09" PRIx64 " div=0x%04x addrDiv=%u key=%s next=%u\n",
               idx, off, raw, roff, div, adiv, kd?"DA":"IA", next);
        printf("           => @slide0: slotVA=0x%" PRIx64 " targetVA=0x%" PRIx64 " key=%s modifier=",
               slotVA, value_add + roff, kd?"DA":"IA");
        if (adiv) printf("blend(0x%" PRIx64 ",0x%04x)\n", slotVA, div);
        else      printf("0x%04x\n", div);
    } else {
        uint32_t high8 = (raw >> 34) & 0xFF;
        printf("      [%2d] @0x%04x REG  raw=0x%016" PRIx64 " roff=0x%09" PRIx64 " high8=0x%02x next=%u\n",
               idx, off, raw, roff, high8, next);
    }
    return next;
}

static void dump_slideinfo(int fd, uint64_t sioff, uint64_t sisize,
                           uint64_t mapAddr, uint64_t mapFileOff){
    struct dyld_cache_slide_info_v3_or_v5 hdr;
    if (pread(fd, &hdr, sizeof hdr, sioff) != (ssize_t)sizeof hdr){ printf("    (slide read failed)\n"); return; }
    printf("    slide-info: version=%u page_size=%u page_starts_count=%u value_add=0x%" PRIx64 "\n",
           hdr.version, hdr.page_size, hdr.page_starts_count, hdr.value_add);
    if (hdr.version != 3 && hdr.version != 5){ printf("    (unhandled slide version %u — dumping raw only)\n", hdr.version); return; }

    // read page_starts[]
    uint64_t psoff = sioff + offsetof(struct dyld_cache_slide_info_v3_or_v5, page_starts);
    uint16_t *ps = slurp(fd, psoff, (uint64_t)hdr.page_starts_count * 2);
    if (!ps){ printf("    (page_starts read failed)\n"); return; }
    int nonrebase=0, first_with=-1;
    for (uint32_t i=0;i<hdr.page_starts_count;i++){ if (ps[i]!=NO_REBASE){ nonrebase++; if(first_with<0) first_with=i; } }
    printf("    pages=%u  with-fixups=%d  no-rebase=%u  first-fixup-page=%d\n",
           hdr.page_starts_count, nonrebase, hdr.page_starts_count-nonrebase, first_with);

    // Walk the chains of up to 2 fixup pages, counting slots + auth, dumping first few.
    int pages_shown=0;
    long total_auth=0, total_slots=0; int histo_pages=0; long sum_slots=0, max_slots=0;
    for (uint32_t i=0;i<hdr.page_starts_count;i++){
        if (ps[i]==NO_REBASE) continue;
        uint64_t pageFileOff = mapFileOff + (uint64_t)i*hdr.page_size;
        uint64_t pageVA      = mapAddr + (uint64_t)i*hdr.page_size;   // at slide 0
        uint8_t *page = slurp(fd, pageFileOff, hdr.page_size);
        if (!page) continue;
        int show = pages_shown < 2;
        if (show) printf("    page[%u] fileOff=0x%" PRIx64 " vmAddr=0x%" PRIx64 " start=0x%x\n",
                         i, pageFileOff, pageVA, ps[i]);
        uint32_t off = ps[i]; long slots=0, auth=0; int shown=0;
        for(;;){
            uint64_t raw = *(uint64_t*)(page + off);
            int isauth = (int)((raw>>63)&1);
            auth += isauth; slots++;
            uint32_t next;
            if (show && shown<6){
                uint64_t slotVA = pageVA + off;
                next = (hdr.version==5)?decode_v5(raw,hdr.value_add,shown,slotVA,off)
                                       :decode_v3(raw,hdr.value_add,shown,slotVA,off);
                shown++;
            }
            else next = (hdr.version==5)? ((uint32_t)(raw>>52)&0x7FF) : ((uint32_t)(raw>>51)&0x7FF);
            if (next==0) break;
            off += next*8;
            if (off >= hdr.page_size){ printf("      !! chain left page (off=0x%x) — CROSS-PAGE\n", off); break; }
            if (slots>4096) { printf("      !! runaway chain\n"); break; }
        }
        if (show) printf("      page[%u]: %ld slots, %ld auth\n", i, slots, auth);
        total_auth+=auth; total_slots+=slots; sum_slots+=slots; if(slots>max_slots)max_slots=slots; histo_pages++;
        free(page);
        if (show) pages_shown++;
        if (histo_pages>=20000) break; // cap work
    }
    printf("    SCANNED %d fixup pages: %ld slots total (%ld auth, %.1f%%), avg %.1f slots/page, max %ld/page\n",
           histo_pages, total_slots, total_auth, total_slots?100.0*total_auth/total_slots:0.0,
           histo_pages?(double)sum_slots/histo_pages:0.0, max_slots);
    free(ps);
}

static void probe_file(const char *path){
    int fd = open(path, O_RDONLY);
    if (fd<0){ printf("== %s : open failed\n", path); return; }
    struct dyld_cache_header h; memset(&h,0,sizeof h);
    if (pread(fd,&h,sizeof h,0)!=(ssize_t)sizeof h){ printf("== %s : header read failed\n", path); close(fd); return; }
    printf("== %s\n", path);
    printf("   magic='%.16s' mappingOffset=0x%x mappingCount=%u  mappingWithSlideOffset=0x%x count=%u\n",
           h.magic, h.mappingOffset, h.mappingCount, h.mappingWithSlideOffset, h.mappingWithSlideCount);
    // uuid@0x58 identifies the exact cache build — quote it when pinning a worked example.
    printf("   uuid=%02X%02X%02X%02X-%02X%02X-%02X%02X-%02X%02X-%02X%02X%02X%02X%02X%02X\n",
           h.uuid[0],h.uuid[1],h.uuid[2],h.uuid[3], h.uuid[4],h.uuid[5], h.uuid[6],h.uuid[7],
           h.uuid[8],h.uuid[9], h.uuid[10],h.uuid[11],h.uuid[12],h.uuid[13],h.uuid[14],h.uuid[15]);
    printf("   sharedRegionStart=0x%" PRIx64 " size=0x%" PRIx64 " maxSlide=0x%" PRIx64 " dyldInCacheMH=0x%" PRIx64 "\n",
           h.sharedRegionStart, h.sharedRegionSize, h.maxSlide, h.dyldInCacheMH);

    // plain mappings
    for (uint32_t i=0;i<h.mappingCount && i<8;i++){
        struct dyld_cache_mapping_info m;
        pread(fd,&m,sizeof m, h.mappingOffset + (uint64_t)i*sizeof m);
        printf("   map[%u]      addr=0x%011" PRIx64 " size=0x%09" PRIx64 " fileOff=0x%09" PRIx64 " %s/%s\n",
               i, m.address, m.size, m.fileOffset, protstr(m.initProt), protstr(m.maxProt));
    }
    // mapping + slide
    for (uint32_t i=0;i<h.mappingWithSlideCount && i<8;i++){
        struct dyld_cache_mapping_and_slide_info m;
        pread(fd,&m,sizeof m, h.mappingWithSlideOffset + (uint64_t)i*sizeof m);
        printf("   slidemap[%u] addr=0x%011" PRIx64 " size=0x%09" PRIx64 " fileOff=0x%09" PRIx64
               " %s/%s slideOff=0x%" PRIx64 " slideSize=0x%" PRIx64 " flags=0x%" PRIx64 "\n",
               i, m.address, m.size, m.fileOffset, protstr(m.initProt), protstr(m.maxProt),
               m.slideInfoFileOffset, m.slideInfoFileSize, m.flags);
        if (m.slideInfoFileOffset && m.slideInfoFileSize)
            dump_slideinfo(fd, m.slideInfoFileOffset, m.slideInfoFileSize, m.address, m.fileOffset);
    }
    close(fd);
}

int main(int argc, char**argv){
    printf("offsetof mappingWithSlideOffset=0x%zx (expect 0x138), mapping_and_slide sz=%zu (expect 56)\n",
           offsetof(struct dyld_cache_header, mappingWithSlideOffset),
           sizeof(struct dyld_cache_mapping_and_slide_info));
    const char *base = "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e";
    if (argc>1){ for(int i=1;i<argc;i++) probe_file(argv[i]); return 0; }
    probe_file(base);
    // subcaches that carry DATA/slide: .01 .05 .09 and the .NN.dylddata files
    const char *subs[] = {".01",".02.dylddata",".05",".06.dylddata",".09",".10.dylddata"};
    char p[512];
    for (unsigned i=0;i<sizeof subs/sizeof subs[0];i++){ snprintf(p,sizeof p,"%s%s",base,subs[i]); probe_file(p); }
    return 0;
}
