#include <sys/event.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <time.h>
static void dump(const char *t, struct kevent *e, int n){ for(int i=0;i<n;i++) printf("%s[%d] ident=%#lx filter=%d flags=%#x fflags=%#x data=%#lx udata=%p\n",t,i,(unsigned long)e[i].ident,e[i].filter,e[i].flags,e[i].fflags,(long)e[i].data,e[i].udata); }
int main(void){
  int fd=kqueue(); struct kevent ev[2]; struct timespec ts={0,0};
  EV_SET(&ev[0],0x1e7e7711,EVFILT_USER,EV_ADD|EV_CLEAR,0,0,0);
  EV_SET(&ev[1],0x1e7e7711,EVFILT_USER,0,NOTE_TRIGGER,0,0);
  int rc=kevent(fd,ev,2,ev,1,&ts); printf("detect rc=%d\n",rc); dump("out",ev,2);
  /* second poll: EV_CLEAR reset? */
  rc=kevent(fd,NULL,0,ev,1,&ts); printf("repoll rc=%d\n",rc);
  /* trigger again with no add */
  EV_SET(&ev[0],0x1e7e7711,EVFILT_USER,0,NOTE_TRIGGER,0,(void*)0x55);
  rc=kevent(fd,ev,1,ev,2,&ts); printf("retrigger rc=%d\n",rc); dump("out",ev,rc>0?rc:0);
  /* EV_DELETE unknown -> EV_ERROR? */
  EV_SET(&ev[0],0x999,EVFILT_USER,EV_DELETE,0,0,0);
  rc=kevent(fd,ev,1,ev,2,&ts); printf("del-unknown rc=%d\n",rc); dump("out",ev,rc>0?rc:0);
  /* pipe read readiness */
  int p[2]; pipe(p); EV_SET(&ev[0],p[0],EVFILT_READ,EV_ADD,0,0,0); rc=kevent(fd,ev,1,ev,2,&ts); printf("pipe-add rc=%d\n",rc);
  write(p[1],"abc",3); rc=kevent(fd,NULL,0,ev,2,&ts); printf("pipe-ready rc=%d\n",rc); dump("out",ev,rc>0?rc:0);
  return 0; }
