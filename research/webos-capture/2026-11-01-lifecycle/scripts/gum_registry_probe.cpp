#include <gum/gum.h>
#include <dlfcn.h>
#include <cstdio>
#include <cstdlib>
#include <exception>
#include <fcntl.h>
#include <unistd.h>
#include <sys/file.h>
#include <signal.h>
#include <pthread.h>
#include <cstdint>
static unsigned registrations,unregistrations,deletions,smoke;
static void *last;
static void enter(GumInvocationContext*c,gpointer kind){
 void*p=gum_invocation_context_get_nth_argument(c,0);
 switch((uintptr_t)kind){
 case 1:last=p;++registrations;fprintf(stderr,"GUM register %p\n",p);break;
 case 2:++unregistrations;fprintf(stderr,"GUM unregister %p\n",p);break;
 case 3:if(p==last){++deletions;fprintf(stderr,"GUM free registered object %p\n",p);}break;
 case 4:++smoke;break;
 }
}
extern "C" __attribute__((noinline)) int smoke_target(int n){volatile int v=n;return v+7;}
static bool hook(GumInterceptor*i,void*p,unsigned kind){if(!p)return false;auto*l=gum_make_call_listener(enter,nullptr,(void*)(uintptr_t)kind,nullptr);auto r=gum_interceptor_attach(i,p,l,nullptr,GUM_ATTACH_FLAGS_NONE);fprintf(stderr,"GUM attach kind=%u result=%d\n",kind,r);return r==GUM_ATTACH_OK;}
int main(int argc,char**argv){
 if(argc!=2)return 2;bool valid=atoi(argv[1])==1;
 sigset_t s;sigemptyset(&s);sigaddset(&s,SIGINT);sigaddset(&s,SIGTERM);pthread_sigmask(SIG_BLOCK,&s,nullptr);
 int lock=open("/tmp/prysm-capture.lock",O_CREAT|O_RDWR|O_NOFOLLOW,0600);if(lock<0||flock(lock,LOCK_EX|LOCK_NB))return 3;
 gum_init_embedded();auto*i=gum_interceptor_obtain();
 if(!hook(i,(void*)smoke_target,4))return 4;
 int(*volatile test)(int)=smoke_target;if(test(5)!=12||smoke!=1)return 5;fprintf(stderr,"GUM smoke passed\n");
 void*lib=dlopen("libvtcapture.so.1",RTLD_NOW|RTLD_GLOBAL);if(!lib){fprintf(stderr,"%s\n",dlerror());return 6;}
 if(!hook(i,dlsym(lib,"_ZN21ResourceManagerClient19registerRmcInstanceEPS_"),1)||!hook(i,dlsym(lib,"_ZN21ResourceManagerClient21unregisterRmcInstanceEPS_"),2)||(!valid&&!hook(i,dlsym(lib,"free"),3)))return 7;
 auto*head=(uintptr_t*)dlsym(lib,"_ZN21ResourceManagerClient6g_rmcsB5cxx11E");
 auto create=(void*(*)())dlsym(lib,"vtCapture_create");auto release=(void(*)(void*))dlsym(lib,"vtCapture_release");if(!head||!create||!release)return 8;
 for(unsigned n=1;n<=3;n++){
 bool threw=false;try{fprintf(stderr,"GUM create begin\n");void*d=create();fprintf(stderr,"GUM create returned\n");if(!d)return 9;usleep(50000);fprintf(stderr,"GUM release begin\n");release(d);fprintf(stderr,"GUM release returned\n");}catch(const std::exception&e){threw=true;fprintf(stderr,"GUM constructor threw %s\n",e.what());}
 bool retained=false;unsigned walked=0;for(auto*node=(uintptr_t*)head[0];node!=head&&walked<8;node=(uintptr_t*)node[0],++walked)if((void*)node[2]==last)retained=true;
 fprintf(stderr,"GUM cycle=%u threw=%d registrations=%u unregistrations=%u deletes=%u list_size=%lu retains_deleted=%d\n",n,threw,registrations,unregistrations,deletions,(unsigned long)head[2],retained);
 if(valid){if(threw||registrations!=n||unregistrations!=n||deletions!=0||head[2]!=0||retained)return 10;}
 else if(!threw||registrations!=n||unregistrations!=0||deletions!=n||!retained)return 11;
 }
 fprintf(stderr,"GUM assertions passed; process exit reclaims diagnostic state\n");_exit(0);
}
