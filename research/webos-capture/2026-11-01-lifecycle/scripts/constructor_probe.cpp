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
#include <luna-service2/lunaservice.h>
static int fds(){int n=0;DIR*d=opendir("/proc/self/fd");if(!d)return -1;while(dirent*e=readdir(d))if(e->d_name[0]!='.')++n;closedir(d);return n-1;}
static bool preflight(){
 void*lib=dlopen("libluna-service2.so.3",RTLD_NOW|RTLD_GLOBAL);if(!lib)std::abort();
 auto init=(bool(*)(LSError*))dlsym(lib,"LSErrorInit");auto free_error=(void(*)(LSError*))dlsym(lib,"LSErrorFree");
 auto reg=(bool(*)(const char*,LSHandle**,LSError*))dlsym(lib,"LSRegister");auto unreg=(bool(*)(LSHandle*,LSError*))dlsym(lib,"LSUnregister");
 if(!init||!free_error||!reg||!unreg)std::abort();
 const char*prefixes[]={"com.webos.rm.client.prysmprobe", "com.webos.service.capture.client.prysmprobe"};
 for(auto prefix:prefixes){char name[128];snprintf(name,sizeof(name),"%s.%d",prefix,getpid());LSError e;init(&e);LSHandle*h=nullptr;
 bool ok=reg(name,&h,&e);if(!ok)fprintf(stderr,"HARNESS preflight denied %s: %s\n",name,e.message?e.message:"?");
 free_error(&e);if(!ok){dlclose(lib);return false;}
 init(&e);if(!unreg(h,&e))std::abort();free_error(&e);
 }dlclose(lib);return true;
}
int main(int argc,char**argv){
 if(argc!=3)return 2;int cycles=atoi(argv[1]),dwell=atoi(argv[2]);if(cycles<1||cycles>50||dwell<0||dwell>1000)return 2;
 sigset_t ss;sigemptyset(&ss);sigaddset(&ss,SIGTERM);sigaddset(&ss,SIGINT);pthread_sigmask(SIG_BLOCK,&ss,0);
 int fd=open("/tmp/prysm-capture.lock",O_CREAT|O_RDWR,0600);if(fd<0||flock(fd,LOCK_EX|LOCK_NB)){perror("capture lock");return 3;}
 void*lib=dlopen("libvtcapture.so.1",RTLD_NOW|RTLD_GLOBAL);if(!lib){puts(dlerror());return 4;}
 auto create=(void*(*)())dlsym(lib,"vtCapture_create");auto release=(void(*)(void*))dlsym(lib,"vtCapture_release");if(!create||!release)return 5;
 fprintf(stderr,"HARNESS baseline fds=%d\n",fds());
 for(int i=0;i<cycles;++i){if(getenv("PRYSM_PREFLIGHT")&&!preflight()){fprintf(stderr,"HARNESS prevented create %d fds=%d\n",i,fds());continue;}try{fprintf(stderr,"HARNESS create %d\n",i);void*p=create();fprintf(stderr,"HARNESS created %p\n",p);if(!p)return 6;if(dwell)usleep(dwell*1000);fprintf(stderr,"HARNESS release %d\n",i);release(p);fprintf(stderr,"HARNESS released %d\n",i);}catch(const std::exception&e){fprintf(stderr,"HARNESS exception %s\n",e.what());}fprintf(stderr,"HARNESS after %d fds=%d\n",i,fds());}
 return 0;
}
