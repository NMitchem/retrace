#include <sys/event.h>
#include <stdio.h>
#include <unistd.h>
int main(void){
  int p[2]; pipe(p); int kq=kqueue(); struct kevent ev; struct timespec ts={0,1};
  EV_SET(&ev,p[1],EVFILT_READ,EV_ADD|EV_ENABLE,0,0,0);
  int rc=kevent(kq,&ev,1,&ev,1,&ts);
  printf("EVFILT_READ on pipe write end, 1ns: rc=%d", rc);
  if(rc>0) printf(" ident=%lu filter=%d flags=%#x data=%ld", (unsigned long)ev.ident, ev.filter, ev.flags, (long)ev.data);
  printf("\n");
  /* stdout (fd 1) as the harness gives it: whatever this process's fd 1 is */
  EV_SET(&ev,1,EVFILT_READ,EV_ADD|EV_ENABLE,0,0,0);
  rc=kevent(kq,&ev,1,&ev,1,&ts);
  fprintf(stderr,"EVFILT_READ on this fd 1, 1ns: rc=%d", rc);
  if(rc>0) fprintf(stderr," ident=%lu filter=%d flags=%#x data=%ld", (unsigned long)ev.ident, ev.filter, ev.flags, (long)ev.data);
  fprintf(stderr,"\n");
  return 0; }
