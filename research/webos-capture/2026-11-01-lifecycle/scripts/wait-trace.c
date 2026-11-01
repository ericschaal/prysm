#define _GNU_SOURCE
#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>
#include <time.h>
#include <sys/syscall.h>
static void logcall(const char *what, void *p, void *q, void *caller) {
 Dl_info d={0}; dladdr(caller,&d); char b[512]; struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t);
 int n=snprintf(b,sizeof(b),"TRACE %ld.%06ld tid=%ld %s %p %p %s+0x%lx\n",t.tv_sec,t.tv_nsec/1000,syscall(SYS_gettid),what,p,q,d.dli_fname?d.dli_fname:"?",(unsigned long)((uintptr_t)caller-(uintptr_t)d.dli_fbase));
 if(n>0) write(2,b,(size_t)n<sizeof(b)?n:sizeof(b)-1);
}
#define REAL(type,name) type real=(type)dlsym(RTLD_NEXT,name); if(!real) _exit(95)
typedef void *(*new_fn)(void);
typedef void *(*loop_fn)(void*,int);
typedef void (*unref_fn)(void*);
void *g_main_context_new(void) { REAL(new_fn,"g_main_context_new"); void*p=real();logcall("context_new",p,0,__builtin_return_address(0));return p; }
void *g_main_loop_new(void*c,int r) { REAL(loop_fn,"g_main_loop_new");void*p=real(c,r);logcall("loop_new",p,c,__builtin_return_address(0));return p; }
void g_main_context_unref(void*p) { REAL(unref_fn,"g_main_context_unref");logcall("context_unref",p,0,__builtin_return_address(0));real(p); }
void g_main_loop_unref(void*p) { REAL(unref_fn,"g_main_loop_unref");logcall("loop_unref",p,0,__builtin_return_address(0));real(p); }
typedef int (*join_fn)(pthread_t,void**);
int pthread_join(pthread_t t,void**p) { REAL(join_fn,"pthread_join");void*c=__builtin_return_address(0);logcall("join_enter",(void*)t,0,c);int r=real(t,p);logcall("join_exit",(void*)t,(void*)(intptr_t)r,c);return r; }
typedef int (*cond_fn)(pthread_cond_t*);
int pthread_cond_signal(pthread_cond_t*p) { REAL(cond_fn,"pthread_cond_signal");logcall("signal",p,0,__builtin_return_address(0));return real(p); }
int pthread_cond_broadcast(pthread_cond_t*p) { REAL(cond_fn,"pthread_cond_broadcast");logcall("broadcast",p,0,__builtin_return_address(0));return real(p); }
typedef int (*wait_fn)(pthread_cond_t*,pthread_mutex_t*);
static int vt_waits;
int pthread_cond_wait(pthread_cond_t*p,pthread_mutex_t*m) {
 REAL(wait_fn,"pthread_cond_wait"); void*c=__builtin_return_address(0);Dl_info d={0};dladdr(c,&d);
 int vt=d.dli_fname && strstr(d.dli_fname,"libvtcapture") && ((uintptr_t)c-(uintptr_t)d.dli_fbase & ~(uintptr_t)1)==0x1153a;
 logcall("wait_enter",p,m,c);
 if(vt) { int n=__sync_add_and_fetch(&vt_waits,1);const char*s=getenv("PRYSM_DELAY_WAIT");if(s && atoi(s)==n){logcall("delay_wait_500ms",p,(void*)(intptr_t)n,c);usleep(500000);logcall("delay_done",p,0,c);} }
 int r=real(p,m);logcall("wait_exit",p,(void*)(intptr_t)r,c);return r;
}
