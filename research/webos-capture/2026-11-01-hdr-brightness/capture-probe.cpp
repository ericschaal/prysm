#include "vtCaptureApi_c.h"
#include <dlfcn.h>
#include <cstdio>
#include <cstdlib>
#include <exception>
#include <dirent.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/file.h>
#include <signal.h>
#include <pthread.h>
#include <stdexcept>
#include <sys/mman.h>
#include <cstdint>
static int entries(const char*p){int n=0;DIR*d=opendir(p);if(!d)return -1;while(dirent*e=readdir(d))if(e->d_name[0]!='.')++n;closedir(d);return n;}
static int maps(){FILE*f=fopen("/proc/self/maps","r");if(!f)return -1;int n=0,c;while((c=fgetc(f))!=EOF)if(c=='\n')++n;fclose(f);return n;}
static void snapshot(int n){fprintf(stderr,"REUSE idle %d fds=%d threads=%d maps=%d\n",n,entries("/proc/self/fd")-1,entries("/proc/self/task"),maps());DIR*d=opendir("/proc/self/fd");if(!d)return;while(dirent*e=readdir(d)){if(e->d_name[0]=='.')continue;char p[128],v[256];snprintf(p,sizeof(p),"/proc/self/fd/%s",e->d_name);ssize_t z=readlink(p,v,sizeof(v)-1);if(z>0){v[z]=0;if(v[0]=='/'&&v[1]=='d'&&v[2]=='e'&&v[3]=='v')fprintf(stderr,"REUSE device %s\n",v);}}closedir(d);}
static void check(const char*what,int result){fprintf(stderr,"REUSE %s result=%d\n",what,result);if(result)throw std::runtime_error(what);}
template<typename T>static T sym(void*lib,const char*name){auto p=(T)dlsym(lib,name);if(!p)throw std::runtime_error(name);return p;}
int main(int argc,char**argv){
 if(argc!=2)return 2;int cycles=3;
 sigset_t ss;sigemptyset(&ss);sigaddset(&ss,SIGINT);sigaddset(&ss,SIGTERM);pthread_sigmask(SIG_BLOCK,&ss,0);
 int lock=open("/tmp/prysm-capture.lock",O_CREAT|O_RDWR|O_NOFOLLOW,0600);if(lock<0||flock(lock,LOCK_EX|LOCK_NB))return 3;
 try{
 void*lib=dlopen("libvtcapture.so.1",RTLD_NOW|RTLD_GLOBAL);if(!lib)throw std::runtime_error(dlerror());
 auto create=sym<VT_DRIVER*(*)()>(lib,"vtCapture_create");
 auto init=sym<int(*)(VT_DRIVER*,const char*,char*)>(lib,"vtCapture_init");
 auto preprocess=sym<int(*)(VT_DRIVER*,char*,_LibVtCaptureProperties*)>(lib,"vtCapture_preprocess");
 auto plane=sym<int(*)(VT_DRIVER*,char*,_LibVtCapturePlaneInfo*)>(lib,"vtCapture_planeInfo");
 auto process=sym<int(*)(VT_DRIVER*,char*)>(lib,"vtCapture_process");
 auto buffer=sym<int(*)(VT_DRIVER*,_LibVtCaptureBufferInfo*)>(lib,"vtCapture_currentCaptureBuffInfo");
 auto stop=sym<int(*)(VT_DRIVER*,char*)>(lib,"vtCapture_stop");
 auto post=sym<int(*)(VT_DRIVER*,char*)>(lib,"vtCapture_postprocess");
 auto finalize=sym<int(*)(VT_DRIVER*,char*)>(lib,"vtCapture_finalize");
 auto release=sym<void(*)(VT_DRIVER*)>(lib,"vtCapture_release");
 auto driver=create();if(!driver)throw std::runtime_error("null driver");snapshot(0);
 for(int n=1;n<=cycles;n++){
  char client[128]={'0','0'};check("init",init(driver,"prysm-webos-capture",client));
  _LibVtCaptureProperties props={};props.dump=n;props.reg.w=320;props.reg.h=180;props.buf_cnt=3;props.frm=30;
  check("preprocess",preprocess(driver,client,&props));check("process",process(driver,client));
  _LibVtCapturePlaneInfo info={};check("plane",plane(driver,client,&info));if(info.planeregion.c!=props.reg.w||info.planeregion.d!=props.reg.h)throw std::runtime_error("unexpected geometry");
  int mem=open("/dev/mem",O_RDONLY|O_SYNC);if(mem<0)throw std::runtime_error("read-only register open");
  auto page=(volatile uint32_t*)mmap(0,4096,PROT_READ,MAP_SHARED,mem,0x18029000);
  if(page==MAP_FAILED)throw std::runtime_error("read-only register map");
  fprintf(stderr,"HDR tap=%d registers=",n);for(int z=0;z<7;z++)fprintf(stderr,"%08x,",page[0xc00/4+z]);fprintf(stderr,"\n");munmap((void*)page,4096);close(mem);
  usleep(150000);
  for(int f=0;f<3;f++) {
   bool got=false;
   for(int k=0;k<100;k++) {
    _LibVtCaptureBufferInfo b={};int code=buffer(driver,&b);
    if(code==0) {
     if(!b.start_addr0||!b.start_addr1||info.stride<320||b.size0<info.stride*180||b.size1<info.stride*90)throw std::runtime_error("bad frame bounds");
     char path[512];snprintf(path,sizeof(path),"%s/tap%d-frame%d.nv12",argv[1],n,f);
     int out=open(path,O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW,0600);if(out<0)throw std::runtime_error("exclusive output");
     FILE *file=fdopen(out,"wb");if(!file)throw std::runtime_error("fdopen");
     for(int y=0;y<180;y++)if(fwrite(b.start_addr0+y*info.stride,1,320,file)!=320)throw std::runtime_error("Y write");
     for(int y=0;y<90;y++)if(fwrite(b.start_addr1+y*info.stride,1,320,file)!=320)throw std::runtime_error("UV write");
     if(fclose(file))throw std::runtime_error("close output");
     fprintf(stderr,"HDR tap=%d frame=%d file=%s\n",n,f,path);got=true;break;
    }
    if(code!=2)check("buffer",code);usleep(10000);
   }
   if(!got)throw std::runtime_error("frame deadline");
   usleep(80000);
  }
  check("stop",stop(driver,client));check("postprocess",post(driver,client));check("finalize",finalize(driver,client));snapshot(n);
 }
 fprintf(stderr,"REUSE final release begin\n");release(driver);fprintf(stderr,"REUSE final release done\n");snapshot(cycles+1);return 0;
 }catch(const std::exception&e){fprintf(stderr,"REUSE exception %s; exit reclaims partial state\n",e.what());_exit(10);}
}
