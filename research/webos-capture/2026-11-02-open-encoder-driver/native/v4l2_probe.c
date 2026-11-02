#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/videodev2.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

/* Synthetic-only, no vendor libraries. Run with an external 12-second watchdog. */
#define NBUF 3
#define MAX_PLANES 2
struct mapping { void *ptr; size_t len; };
struct queue {
    enum v4l2_buf_type type;
    unsigned planes, count;
    int streaming;
    struct v4l2_pix_format_mplane fmt;
    struct mapping mem[NBUF][MAX_PLANES];
    int busy[NBUF];
};
static volatile sig_atomic_t interrupted;
static uint64_t origin;
static uint64_t now_us(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t)t.tv_sec * 1000000u + (uint64_t)t.tv_nsec / 1000u;
}
static void stopped(int sig) { interrupted = sig; }
static int call(int fd, unsigned long request, void *arg) {
    int r;
    do { r = ioctl(fd, request, arg); } while (r < 0 && errno == EINTR && !interrupted);
    return r;
}
static void error_at(const char *stage) {
    int e = errno;
    fprintf(stderr, "{\"event\":\"error\",\"stage\":\"%s\",\"errno\":%d,\"t_us\":%llu}\n",
        stage, e, (unsigned long long)(now_us()-origin));
}
static int configure(int fd, struct queue *q, uint32_t fourcc, int set) {
    struct v4l2_format f;
    memset(&f, 0, sizeof(f));
    f.type=q->type;
    f.fmt.pix_mp.width=640;
    f.fmt.pix_mp.height=360;
    f.fmt.pix_mp.pixelformat=fourcc;
    f.fmt.pix_mp.field=V4L2_FIELD_NONE;
    f.fmt.pix_mp.num_planes=q->planes;
    f.fmt.pix_mp.colorspace=V4L2_COLORSPACE_REC709;
    if(q->type==V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
        /* Standard compressed size request; the driver may adjust it. */
        f.fmt.pix_mp.plane_fmt[0].sizeimage=1024*1024;
    } else {
        /* Standard stride preference, not a private ABI assumption. */
        for(unsigned p=0;p<q->planes;p++) {
            f.fmt.pix_mp.plane_fmt[p].bytesperline=768;
            f.fmt.pix_mp.plane_fmt[p].sizeimage=768*(360/(p?2:1));
        }
    }
    if (set && call(fd, VIDIOC_S_FMT, &f)<0) return -1;
    memset(&f,0,sizeof(f)); f.type=q->type;
    if (call(fd, VIDIOC_G_FMT, &f)<0) return -1;
    q->fmt=f.fmt.pix_mp;
    fprintf(stderr,"{\"event\":\"format\",\"type\":%u,\"width\":%u,\"height\":%u,\"fourcc\":%u,\"planes\":%u,\"colorspace\":%u}\n",
        q->type,q->fmt.width,q->fmt.height,q->fmt.pixelformat,q->fmt.num_planes,q->fmt.colorspace);
    if(q->fmt.pixelformat!=fourcc || q->fmt.num_planes!=q->planes) { errno=EINVAL; return -1; }
    /* Initial CAPTURE geometry depends on the later OUTPUT negotiation. */
    int provisional=set && q->type==V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    if(!provisional && (
       q->fmt.width<640 || q->fmt.width>2048 || q->fmt.height<360 || q->fmt.height>2048 ||
       (q->fmt.width&1) || (q->fmt.height&1))) { errno=EINVAL; return -1; }
    for(unsigned p=0;p<q->planes;p++) {
        fprintf(stderr,"{\"event\":\"plane_format\",\"type\":%u,\"plane\":%u,\"stride\":%u,\"sizeimage\":%u}\n",
            q->type,p,q->fmt.plane_fmt[p].bytesperline,q->fmt.plane_fmt[p].sizeimage);
        if((!provisional && !q->fmt.plane_fmt[p].sizeimage) || q->fmt.plane_fmt[p].sizeimage>32*1024*1024) { errno=EINVAL; return -1; }
        if(q->type==V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE) {
            uint64_t needed=(uint64_t)q->fmt.plane_fmt[p].bytesperline*(q->fmt.height/(p?2:1));
            if(q->fmt.plane_fmt[p].bytesperline<q->fmt.width || needed>q->fmt.plane_fmt[p].sizeimage) { errno=EINVAL; return -1; }
        }
    }
    return 0;
}
static void control(int fd, uint32_t id, int value) {
    struct v4l2_control c={.id=id,.value=value};
    int r=call(fd,VIDIOC_S_CTRL,&c), e=r<0?errno:0;
    struct v4l2_control got={.id=id};
    int g=call(fd,VIDIOC_G_CTRL,&got);
    fprintf(stderr,"{\"event\":\"control\",\"id\":%u,\"requested\":%d,\"set_errno\":%d,\"get_ok\":%d,\"actual\":%d}\n",id,value,e,g==0,got.value);
}
static int allocate(int fd, struct queue *q) {
    struct v4l2_requestbuffers r={.count=NBUF,.type=q->type,.memory=V4L2_MEMORY_MMAP};
    if(call(fd,VIDIOC_REQBUFS,&r)<0) return -1;
    if(r.count<2 || r.count>NBUF) { errno=EINVAL; return -1; }
    q->count=r.count;
    for(unsigned i=0;i<q->count;i++) {
        struct v4l2_plane p[MAX_PLANES]={0};
        struct v4l2_buffer b={.index=i,.type=q->type,.memory=V4L2_MEMORY_MMAP,.length=q->planes,.m.planes=p};
        if(call(fd,VIDIOC_QUERYBUF,&b)<0) return -1;
        if(b.length!=q->planes) { errno=EINVAL; return -1; }
        for(unsigned k=0;k<q->planes;k++) {
            fprintf(stderr,"{\"event\":\"map\",\"type\":%u,\"index\":%u,\"plane\":%u,\"length\":%u,\"offset\":%u}\n",q->type,i,k,p[k].length,p[k].m.mem_offset);
            if(p[k].length<q->fmt.plane_fmt[k].sizeimage || p[k].length>32*1024*1024) { errno=EINVAL; return -1; }
            void *ptr=mmap(NULL,p[k].length,PROT_READ|PROT_WRITE,MAP_SHARED,fd,p[k].m.mem_offset);
            if(ptr==MAP_FAILED) return -1;
            q->mem[i][k]=(struct mapping){ptr,p[k].length};
        }
    }
    return 0;
}
static void fill(struct queue *q, unsigned i, unsigned frame) {
    for(unsigned p=0;p<q->planes;p++) memset(q->mem[i][p].ptr,p?128:16,q->mem[i][p].len);
    unsigned w=q->fmt.width,h=q->fmt.height,ys=q->fmt.plane_fmt[0].bytesperline,uvs=q->fmt.plane_fmt[1].bytesperline;
    unsigned char *y=q->mem[i][0].ptr,*uv=q->mem[i][1].ptr;
    static const unsigned char u[4]={90,110,150,170},v[4]={170,150,110,90};
    for(unsigned row=0;row<h;row++) for(unsigned x=0;x<w;x++)
        y[row*ys+x]=(unsigned char)(32+20*((x/80+frame)%8));
    for(unsigned row=0;row<h/2;row++) for(unsigned x=0;x<w;x+=2) {
        unsigned bar=(x/160+frame)%4;
        uv[row*uvs+x]=u[bar]; uv[row*uvs+x+1]=v[bar];
    }
}
static int self_check(void) {
    struct queue q={.planes=2};
    q.fmt.width=640;q.fmt.height=360;
    unsigned char *allocation[2]={NULL,NULL};
    int result=1;
    for(unsigned p=0;p<2;p++) {
        q.fmt.plane_fmt[p].bytesperline=768;
        q.mem[0][p].len=768*(360/(p?2:1))+128;
        allocation[p]=malloc(q.mem[0][p].len+64);
        if(!allocation[p]) goto done;
        memset(allocation[p],0xa5,q.mem[0][p].len+64);
        q.mem[0][p].ptr=allocation[p]+32;
    }
    for(unsigned frame=0;frame<16;frame++) {
        fill(&q,0,frame);
        for(unsigned p=0;p<2;p++) {
            unsigned char *data=q.mem[0][p].ptr;
            unsigned rows=360/(p?2:1);
            for(unsigned i=0;i<32;i++) if(allocation[p][i]!=0xa5 || data[q.mem[0][p].len+i]!=0xa5) goto done;
            for(size_t i=0;i<q.mem[0][p].len;i++) {
                unsigned row=i/768,x=i%768;
                unsigned char expected=p?128:16;
                if(row<rows && x<640) {
                    if(!p) expected=32+20*((x/80+frame)%8);
                    else {
                        const unsigned char u[4]={90,110,150,170},v[4]={170,150,110,90};
                        unsigned bar=((x/2*2)/160+frame)%4;
                        expected=(x&1)?v[bar]:u[bar];
                    }
                }
                if(data[i]!=expected) goto done;
            }
        }
    }
    result=0;
done:
    free(allocation[0]);free(allocation[1]);
    fprintf(stderr,"{\"event\":\"self_check\",\"result\":%d,\"frames\":16,\"stride\":768,\"guards\":32}\n",result);
    return result;
}
static int enqueue(int fd, struct queue *q, unsigned index, unsigned frame) {
    struct v4l2_plane p[MAX_PLANES]={0};
    struct v4l2_buffer b={.index=index,.type=q->type,.memory=V4L2_MEMORY_MMAP,.length=q->planes,.m.planes=p};
    for(unsigned k=0;k<q->planes;k++) {
        p[k].length=q->mem[index][k].len;
        if(q->type==V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE) p[k].bytesused=q->fmt.plane_fmt[k].sizeimage;
    }
    if(q->type==V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE) {
        uint64_t stamp=1000000u+(uint64_t)frame*33333u;
        b.timestamp.tv_sec=stamp/1000000u; b.timestamp.tv_usec=stamp%1000000u;
        b.field=V4L2_FIELD_NONE;
    }
    uint64_t start=now_us();
    if(call(fd,VIDIOC_QBUF,&b)<0) return -1;
    q->busy[index]=1;
    if(q->type==V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE)
        fprintf(stderr,"{\"event\":\"submit\",\"frame\":%u,\"index\":%u,\"t_us\":%llu,\"ioctl_us\":%llu,\"pts_us\":%llu}\n",frame,index,(unsigned long long)(start-origin),(unsigned long long)(now_us()-start),(unsigned long long)b.timestamp.tv_sec*1000000u+b.timestamp.tv_usec);
    return 0;
}
static int write_all(int fd, const void *data, size_t length) {
    const unsigned char *p=data;
    while(length) {
        ssize_t n=write(fd,p,length);
        if(n<0 && errno==EINTR && !interrupted) continue;
        if(n<=0) return -1;
        p+=n; length-=(size_t)n;
    }
    return 0;
}
static int drain(int fd, struct queue *q, int out, unsigned *count, uint64_t *last_data, uint64_t *bytes) {
    for(unsigned attempt=0;attempt<NBUF;attempt++) {
        struct v4l2_plane p[MAX_PLANES]={0};
        struct v4l2_buffer b={.type=q->type,.memory=V4L2_MEMORY_MMAP,.length=q->planes,.m.planes=p};
        if(call(fd,VIDIOC_DQBUF,&b)<0) { if(errno==EAGAIN) return 0; return -1; }
        if(b.index>=q->count || !q->busy[b.index] || b.length!=q->planes) { errno=EPROTO; return -1; }
        q->busy[b.index]=0;
        fprintf(stderr,"{\"event\":\"dequeue\",\"type\":%u,\"index\":%u,\"sequence\":%u,\"flags\":%u,\"t_us\":%llu,\"pts_us\":%llu,\"bytesused\":%u,\"data_offset\":%u}\n",q->type,b.index,b.sequence,b.flags,(unsigned long long)(now_us()-origin),(unsigned long long)b.timestamp.tv_sec*1000000u+b.timestamp.tv_usec,p[0].bytesused,p[0].data_offset);
        if(b.flags&V4L2_BUF_FLAG_ERROR) { errno=EIO; return -1; }
        for(unsigned k=0;k<q->planes;k++) if(p[k].bytesused>q->mem[b.index][k].len || p[k].data_offset>p[k].bytesused) { errno=EOVERFLOW; return -1; }
        if(q->type==V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
            size_t n=p[0].bytesused-p[0].data_offset;
            if(n) {
                if(write_all(out,(unsigned char *)q->mem[b.index][0].ptr+p[0].data_offset,n)<0) return -1;
                *bytes+=n; (*count)++; *last_data=now_us();
            }
            if(enqueue(fd,q,b.index,0)<0) return -1;
        } else (*count)++;
    }
    return 0;
}
static void release_queue(int fd, struct queue *q) {
    if(fd>=0 && q->streaming) {
        int result=call(fd,VIDIOC_STREAMOFF,&q->type);
        fprintf(stderr,"{\"event\":\"streamoff\",\"type\":%u,\"result\":%d,\"errno\":%d}\n",q->type,result,result<0?errno:0);
        q->streaming=0;
    }
    for(unsigned i=0;i<NBUF;i++) for(unsigned p=0;p<MAX_PLANES;p++)
        if(q->mem[i][p].ptr) munmap(q->mem[i][p].ptr,q->mem[i][p].len);
    if(fd>=0 && q->count) {
        struct v4l2_requestbuffers r={.count=0,.type=q->type,.memory=V4L2_MEMORY_MMAP};
        int result=call(fd,VIDIOC_REQBUFS,&r);
        fprintf(stderr,"{\"event\":\"release\",\"type\":%u,\"result\":%d,\"errno\":%d}\n",q->type,result,result<0?errno:0);
    }
}
int main(int argc,char **argv) {
    setvbuf(stderr,NULL,_IONBF,0); origin=now_us();
    if(argc==2 && !strcmp(argv[1],"--self-check")) return self_check();
    if(argc!=3) { fprintf(stderr,"usage: %s output.h264 frames[1..90]\n",argv[0]); return 2; }
    char *end; unsigned long wanted=strtoul(argv[2],&end,10);
    if(*end || wanted<1 || wanted>90) return 2;
    struct sigaction sa; memset(&sa,0,sizeof(sa)); sa.sa_handler=stopped;
    sigaction(SIGTERM,&sa,NULL); sigaction(SIGINT,&sa,NULL); sigaction(SIGALRM,&sa,NULL);
    alarm(9);
    int fd=-1,lock=-1,out=-1,result=1;
    unsigned sent=0,returned=0,received=0;
    uint64_t bytes=0,last_data=origin,first_submit=0,last_submit=0,finished_inputs=0;
    const char *stage="lock";
    struct queue input={.type=V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE,.planes=2};
    struct queue capture={.type=V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,.planes=1};
    lock=open("/tmp/prysm-capture.lock",O_RDWR|O_CREAT|O_CLOEXEC|O_NOFOLLOW,0600);
    if(lock<0 || flock(lock,LOCK_EX|LOCK_NB)<0) goto fail;
    fprintf(stderr,"{\"event\":\"lock_acquired\"}\n");
    stage="open_device"; fd=open("/dev/video24",O_RDWR|O_NONBLOCK|O_CLOEXEC); if(fd<0) goto fail;
    stage="querycap";
    struct v4l2_capability caps; memset(&caps,0,sizeof(caps));
    if(call(fd,VIDIOC_QUERYCAP,&caps)<0) goto fail;
    unsigned capabilities=(caps.capabilities&V4L2_CAP_DEVICE_CAPS)?caps.device_caps:caps.capabilities;
    fprintf(stderr,"{\"event\":\"capabilities\",\"driver\":\"%.16s\",\"caps\":%u}\n",caps.driver,capabilities);
    if(!(capabilities&V4L2_CAP_STREAMING) || !(capabilities&V4L2_CAP_VIDEO_M2M_MPLANE)) {errno=ENOTSUP;goto fail;}
    stage="capture_format"; if(configure(fd,&capture,V4L2_PIX_FMT_H264,1)<0) goto fail;
    stage="output_format"; if(configure(fd,&input,V4L2_PIX_FMT_NV12M,1)<0) goto fail;
    control(fd,V4L2_CID_MPEG_VIDEO_H264_PROFILE,V4L2_MPEG_VIDEO_H264_PROFILE_BASELINE);
    control(fd,V4L2_CID_MPEG_VIDEO_H264_LEVEL,V4L2_MPEG_VIDEO_H264_LEVEL_3_0);
    control(fd,V4L2_CID_MPEG_VIDEO_BITRATE,2000000);
    control(fd,V4L2_CID_MPEG_VIDEO_GOP_SIZE,30);
    control(fd,V4L2_CID_MPEG_VIDEO_B_FRAMES,0);
    control(fd,V4L2_CID_MPEG_VIDEO_HEADER_MODE,V4L2_MPEG_VIDEO_HEADER_MODE_JOINED_WITH_1ST_FRAME);
    struct v4l2_streamparm sp; memset(&sp,0,sizeof(sp)); sp.type=input.type;
    sp.parm.output.timeperframe.numerator=1; sp.parm.output.timeperframe.denominator=30;
    int sr=call(fd,VIDIOC_S_PARM,&sp);
    fprintf(stderr,"{\"event\":\"streamparm\",\"result\":%d,\"errno\":%d,\"numerator\":%u,\"denominator\":%u}\n",sr,sr<0?errno:0,sp.parm.output.timeperframe.numerator,sp.parm.output.timeperframe.denominator);
    stage="final_capture_format"; if(configure(fd,&capture,V4L2_PIX_FMT_H264,0)<0) goto fail;
    stage="final_output_format"; if(configure(fd,&input,V4L2_PIX_FMT_NV12M,0)<0) goto fail;
    stage="allocate_output"; if(allocate(fd,&input)<0) goto fail;
    stage="allocate_capture"; if(allocate(fd,&capture)<0) goto fail;
    stage="open_bitstream"; out=open(argv[1],O_WRONLY|O_CREAT|O_EXCL|O_CLOEXEC,0600); if(out<0) goto fail;
    for(unsigned i=0;i<capture.count;i++) {stage="queue_capture"; if(enqueue(fd,&capture,i,0)<0) goto fail;}
    fill(&input,0,0); stage="queue_first"; first_submit=last_submit=now_us();
    if(enqueue(fd,&input,0,0)<0) goto fail;
    sent=1; last_data=now_us();
    stage="streamon_capture"; if(call(fd,VIDIOC_STREAMON,&capture.type)<0) goto fail; capture.streaming=1;
    stage="streamon_output"; if(call(fd,VIDIOC_STREAMON,&input.type)<0) goto fail; input.streaming=1;
    fprintf(stderr,"{\"event\":\"streaming\",\"t_us\":%llu,\"first_input_wait_us\":250000}\n",(unsigned long long)(now_us()-origin));
    while(!interrupted) {
        stage="dequeue_capture"; if(drain(fd,&capture,out,&received,&last_data,&bytes)<0) goto fail;
        stage="dequeue_output"; if(drain(fd,&input,out,&returned,&last_data,&bytes)<0) goto fail;
        uint64_t now=now_us();
        if(sent==wanted && returned==wanted && !finished_inputs) finished_inputs=now;
        if(finished_inputs && received>=wanted && now-last_data>=300000) {result=0;break;}
        if(now-origin>8000000 || now-last_data>1500000) {stage="no_output_or_total_deadline";errno=ETIMEDOUT;goto fail;}
        if(sent<wanted && now-first_submit>=250000 && now-last_submit>=33333) {
            for(unsigned i=0;i<input.count;i++) if(!input.busy[i]) {
                fill(&input,i,sent); stage="queue_input";
                if(enqueue(fd,&input,i,sent)<0) goto fail;
                sent++;last_submit=now_us();break;
            }
        }
        /* POLLOUT remains ready with spare raw buffers; do not spin on it. */
        struct pollfd pfd={.fd=fd,.events=POLLIN|POLLPRI};
        int pr=poll(&pfd,1,10);
        if(pr<0 && errno!=EINTR) {stage="poll";goto fail;}
        if(pr>0 && (pfd.revents&(POLLHUP|POLLNVAL|POLLERR))) {stage="poll_device_error";errno=EIO;goto fail;}
    }
    if(interrupted) {stage="signal";errno=EINTR;goto fail;}
    goto done;
fail:
    error_at(stage);
done:
    /* Stop both queues before unmapping either; driver ownership ends here. */
    if(fd>=0) {
        int stop_failed=0;
        if(input.streaming) {
            int r=call(fd,VIDIOC_STREAMOFF,&input.type);
            fprintf(stderr,"{\"event\":\"streamoff\",\"type\":%u,\"result\":%d,\"errno\":%d}\n",input.type,r,r<0?errno:0);
            if(r<0) {result=1;stop_failed=1;}
            input.streaming=0;
        }
        if(capture.streaming) {
            int r=call(fd,VIDIOC_STREAMOFF,&capture.type);
            fprintf(stderr,"{\"event\":\"streamoff\",\"type\":%u,\"result\":%d,\"errno\":%d}\n",capture.type,r,r<0?errno:0);
            if(r<0) {result=1;stop_failed=1;}
            capture.streaming=0;
        }
        if(stop_failed) {close(fd);fd=-1;}
    }
    release_queue(fd,&input);release_queue(fd,&capture);
    if(out>=0) close(out);
    if(fd>=0) close(fd);
    if(lock>=0) close(lock);
    fprintf(stderr,"{\"event\":\"summary\",\"result\":%d,\"requested\":%lu,\"submitted\":%u,\"returned\":%u,\"capture_buffers\":%u,\"bytes\":%llu,\"t_us\":%llu,\"signal\":%d}\n",result,wanted,sent,returned,received,(unsigned long long)bytes,(unsigned long long)(now_us()-origin),interrupted);
    return result;
}
