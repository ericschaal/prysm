#define _GNU_SOURCE
#include <gst/gst.h>
#include <gst/app/gstappsrc.h>
#include <gst/app/gstappsink.h>
#include <gst/video/video.h>
#include <gst/allocators/gstdmabuf.h>
#include <sys/file.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

enum { W=640, H=360, STRIDE=768, SIZE=STRIDE*H*3/2 };
struct allocation { uint64_t len; uint32_t fd, flags; uint64_t heap_flags; };
static gint64 origin;
static unsigned collected;
static FILE *stream;
static const char *directory;
static double ms(void) { return (g_get_monotonic_time()-origin)/1000.0; }
#define CHECK(x) do { if (!(x)) { fprintf(stderr,"FAIL %s:%d %s errno=%d\n",__FILE__,__LINE__,#x,errno); goto cleanup; } } while(0)

static GstPadProbeReturn trace(GstPad *pad, GstPadProbeInfo *info, gpointer label) {
    (void)pad;
    if (GST_PAD_PROBE_INFO_TYPE(info)&GST_PAD_PROBE_TYPE_BUFFER) {
        GstBuffer *b=GST_PAD_PROBE_INFO_BUFFER(info);
        printf("%.3f %s bytes=%zu pts=%" G_GUINT64_FORMAT " flags=%u\n",ms(),(char*)label,gst_buffer_get_size(b),GST_BUFFER_PTS(b),GST_BUFFER_FLAGS(b));
    } else if (GST_PAD_PROBE_INFO_TYPE(info)&GST_PAD_PROBE_TYPE_EVENT_DOWNSTREAM) {
        GstEvent *event=GST_PAD_PROBE_INFO_EVENT(info);
        if (GST_EVENT_TYPE(event)==GST_EVENT_CAPS) {
            GstCaps *caps=NULL; gst_event_parse_caps(event,&caps);
            char *s=gst_caps_to_string(caps);
            fprintf(stderr,"CAPS %s %s\n",(char*)label,s); g_free(s);
        }
    }
    return GST_PAD_PROBE_OK;
}

static int collect(GstAppSink *sink, GstClockTime wait) {
    GstSample *sample=gst_app_sink_try_pull_sample(sink,wait);
    if (!sample) return 0;
    GstBuffer *b=gst_sample_get_buffer(sample); GstMapInfo map;
    if (!b || !gst_buffer_map(b,&map,GST_MAP_READ)) { gst_sample_unref(sample); return -1; }
    printf("%.3f collect index=%u bytes=%zu\n",ms(),collected,map.size);
    char path[512]; int len=snprintf(path,sizeof(path),"%s/frame-%03u.au",directory,collected);
    FILE *file=(len>0 && (size_t)len<sizeof(path))?fopen(path,"wb"):NULL;
    int ok=file && map.size && map.size<=8*1024*1024;
    if(ok) ok=fwrite(map.data,1,map.size,file)==map.size && fwrite(map.data,1,map.size,stream)==map.size;
    if(file && fclose(file)) ok=0;
    gst_buffer_unmap(b,&map); gst_sample_unref(sample);
    if(!ok) return -1;
    collected++; return 1;
}

int main(int argc,char **argv) {
    if(argc==2 && !strcmp(argv[1],"--self-check")) {
        if(SIZE!=414720 || STRIDE%192 || sizeof(struct allocation)!=24) return 1;
        for(unsigned i=0;i<45;i++) if(32+(i%16)*8>235) return 1;
        puts("layout and pattern checks passed"); return 0;
    }
    if(argc!=4 || (strcmp(argv[1],"parsed") && strcmp(argv[1],"direct"))) return 2;
    char *end; long count=strtol(argv[3],&end,10);
    if(*end || count<2 || count>45) return 2;
    directory=argv[2]; setvbuf(stdout,NULL,_IOLBF,0); origin=g_get_monotonic_time();
    int result=1,lock=-1,heap=-1,fd[2]={-1,-1};
    void *mapping[2]={MAP_FAILED,MAP_FAILED}; GstMemory *memory[2]={NULL,NULL};
    GstElement *pipeline=NULL,*source=NULL,*sink=NULL; GstAllocator *allocator=NULL;
    GstBus *bus=NULL; GError *error=NULL;
    lock=open("/tmp/prysm-capture.lock",O_RDWR|O_CREAT|O_CLOEXEC|O_NOFOLLOW,0600);
    CHECK(lock>=0 && flock(lock,LOCK_EX|LOCK_NB)==0);
    gst_init(NULL,NULL);
    char output[512]; CHECK(snprintf(output,sizeof(output),"%s/output.h264",directory)<(int)sizeof(output));
    stream=fopen(output,"wb"); CHECK(stream);
    char *graph=g_strdup_printf("appsrc name=input is-live=true do-timestamp=true format=time block=false caps=\"video/x-raw(memory:DMABuf),format=NV12,width=640,height=360,framerate=30/1,colorimetry=bt709\" ! rvenc name=encoder target-bitrate=2000000 ! %s appsink name=output async=false sync=false max-buffers=4 drop=false wait-on-eos=false",!strcmp(argv[1],"parsed")?"h264parse name=parser config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au !":"");
    pipeline=gst_parse_launch(graph,&error); g_free(graph);
    if(error) { fprintf(stderr,"parse: %s\n",error->message); g_error_free(error); goto cleanup; }
    CHECK(pipeline);
    source=gst_bin_get_by_name(GST_BIN(pipeline),"input"); sink=gst_bin_get_by_name(GST_BIN(pipeline),"output"); CHECK(source && sink);
    const char *names[]={"encoder","encoder","parser"},*pads[]={"sink","src","src"},*labels[]={"encoder-in","encoder-out","parser-out"};
    for(unsigned i=0;i<3;i++) {
        GstElement *e=gst_bin_get_by_name(GST_BIN(pipeline),names[i]);
        if(!e) { CHECK(i==2); continue; }
        GstPad *pad=gst_element_get_static_pad(e,pads[i]); gst_object_unref(e); CHECK(pad);
        gst_pad_add_probe(pad,GST_PAD_PROBE_TYPE_BUFFER|GST_PAD_PROBE_TYPE_EVENT_DOWNSTREAM,trace,(gpointer)labels[i],NULL); gst_object_unref(pad);
    }
    bus=gst_element_get_bus(pipeline); allocator=gst_dmabuf_allocator_new(); CHECK(bus && allocator);
    heap=open("/dev/dma_heap/dvr",O_RDWR|O_CLOEXEC); CHECK(heap>=0);
    for(unsigned i=0;i<2;i++) {
        struct allocation req={.len=SIZE,.flags=O_RDWR|O_CLOEXEC}; CHECK(ioctl(heap,0xc0184800,&req)==0);
        fd[i]=(int)req.fd; CHECK(fd[i]>=0);
        mapping[i]=mmap(NULL,SIZE,PROT_READ|PROT_WRITE,MAP_SHARED,fd[i],0); CHECK(mapping[i]!=MAP_FAILED);
        int owned=dup(fd[i]); CHECK(owned>=0);
        memory[i]=gst_dmabuf_allocator_alloc(allocator,owned,SIZE);
        if(!memory[i]) { close(owned); goto cleanup; }
    }
    CHECK(gst_element_set_state(pipeline,GST_STATE_PLAYING)!=GST_STATE_CHANGE_FAILURE);
    for(long index=0;index<count;index++) {
        unsigned slot=(unsigned)index%2;
        CHECK(gst_mini_object_is_writable(GST_MINI_OBJECT(memory[slot])));
        uint64_t sync=2; CHECK(ioctl(fd[slot],0x40086200,&sync)==0);
        unsigned char *data=mapping[slot]; memset(data,32+(index%16)*8,STRIDE*H);
        for(unsigned j=STRIDE*H;j<SIZE;j+=2) { data[j]=index%2?160:96; data[j+1]=index%2?96:160; }
        sync=6; CHECK(ioctl(fd[slot],0x40086200,&sync)==0);
        GstBuffer *buffer=gst_buffer_new(); CHECK(buffer);
        gst_buffer_append_memory(buffer,gst_memory_ref(memory[slot]));
        gsize offsets[4]={0,STRIDE*H,0,0}; gint strides[4]={STRIDE,STRIDE,0,0};
        if(!gst_buffer_add_video_meta_full(buffer,0,GST_VIDEO_FORMAT_NV12,W,H,2,offsets,strides)) { gst_buffer_unref(buffer); goto cleanup; }
        printf("%.3f submit index=%ld\n",ms(),index);
        CHECK(gst_app_src_push_buffer(GST_APP_SRC(source),buffer)==GST_FLOW_OK);
        gint64 until=g_get_monotonic_time()+(index==0?250000:33333);
        for(;;) {
            gint64 remaining=until-g_get_monotonic_time();
            if(remaining<=0) break;
            GstClockTime timeout=(GstClockTime)remaining*1000;
            CHECK(collect(GST_APP_SINK(sink),timeout)>=0);
        }
        if(index==0) printf("%.3f single-frame-window outputs=%u\n",ms(),collected);
        GstMessage *msg=gst_bus_pop_filtered(bus,GST_MESSAGE_ERROR);
        if(msg) { GError *err=NULL; gchar *debug=NULL; gst_message_parse_error(msg,&err,&debug); fprintf(stderr,"BUS %s %s\n",err->message,debug?debug:""); g_clear_error(&err); g_free(debug); gst_message_unref(msg); goto cleanup; }
    }
    printf("%.3f eos-submit outputs=%u\n",ms(),collected);
    CHECK(gst_app_src_end_of_stream(GST_APP_SRC(source))==GST_FLOW_OK);
    for(unsigned i=0;i<30 && !gst_app_sink_is_eos(GST_APP_SINK(sink));i++) CHECK(collect(GST_APP_SINK(sink),50*GST_MSECOND)>=0);
    CHECK(gst_app_sink_is_eos(GST_APP_SINK(sink)));
    CHECK(collected==(unsigned)count);
    result=0;
cleanup:
    if(pipeline) gst_element_set_state(pipeline,GST_STATE_NULL);
    for(unsigned i=0;i<2;i++) { if(memory[i]) gst_memory_unref(memory[i]); if(mapping[i]!=MAP_FAILED) munmap(mapping[i],SIZE); if(fd[i]>=0) close(fd[i]); }
    if(bus) gst_object_unref(bus); if(source) gst_object_unref(source); if(sink) gst_object_unref(sink);
    if(allocator) gst_object_unref(allocator); if(pipeline) gst_object_unref(pipeline);
    if(stream && fclose(stream)) result=1; if(heap>=0) close(heap); if(lock>=0) close(lock);
    printf("%.3f finish status=%d outputs=%u\n",ms(),result,collected); return result;
}
