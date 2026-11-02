
/private/tmp/prysm-color-kernel.elf:	file format elf64-littleaarch64

Disassembly of section .text:

ffffffc010180000 <.text>:
ffffffc010a95584: a9bb7bfd     	stp	x29, x30, [sp, #-0x50]!
ffffffc010a95588: 910003fd     	mov	x29, sp
ffffffc010a9558c: a90153f3     	stp	x19, x20, [sp, #0x10]
ffffffc010a95590: a9025bf5     	stp	x21, x22, [sp, #0x20]
ffffffc010a95594: a90363f7     	stp	x23, x24, [sp, #0x30]
ffffffc010a95598: a9046bf9     	stp	x25, x26, [sp, #0x40]
ffffffc010a9559c: b0006cb3     	adrp	x19, 0xffffffc01182a000 <.text+0x16aa000>
ffffffc010a955a0: 91382273     	add	x19, x19, #0xe08
ffffffc010a955a4: d50320ff     	xpaclri
ffffffc010a955a8: 2a0003f5     	mov	w21, w0
ffffffc010a955ac: aa1e03e0     	mov	x0, x30
ffffffc010a955b0: 97dc11fe     	bl	0xffffffc010199da8 <.text+0x19da8>
ffffffc010a955b4: b9421260     	ldr	w0, [x19, #0x210]
ffffffc010a955b8: 51000c00     	sub	w0, w0, #0x3
ffffffc010a955bc: 7100041f     	cmp	w0, #0x1
ffffffc010a955c0: 54000c28     	b.hi	0xffffffc010a95744 <.text+0x915744>
ffffffc010a955c4: 79444261     	ldrh	w1, [x19, #0x220]
ffffffc010a955c8: 91088273     	add	x19, x19, #0x220
ffffffc010a955cc: 79400660     	ldrh	w0, [x19, #0x2]
ffffffc010a955d0: 12003821     	and	w1, w1, #0x7fff
ffffffc010a955d4: 53103800     	ubfiz	w0, w0, #16, #15
ffffffc010a955d8: 2a010000     	orr	w0, w0, w1
ffffffc010a955dc: 928c7f61     	mov	x1, #-0x63fc    // =-25596
ffffffc010a955e0: f2a30041     	movk	x1, #0x1802, lsl #16
ffffffc010a955e4: b9000020     	str	w0, [x1]
ffffffc010a955e8: d503221f     	esb
ffffffc010a955ec: 79400e60     	ldrh	w0, [x19, #0x6]
ffffffc010a955f0: 79400a61     	ldrh	w1, [x19, #0x4]
ffffffc010a955f4: 53103800     	ubfiz	w0, w0, #16, #15
ffffffc010a955f8: 12003821     	and	w1, w1, #0x7fff
ffffffc010a955fc: 2a010000     	orr	w0, w0, w1
ffffffc010a95600: 928c7ee1     	mov	x1, #-0x63f8    // =-25592
ffffffc010a95604: f2a30041     	movk	x1, #0x1802, lsl #16
ffffffc010a95608: b9000020     	str	w0, [x1]
ffffffc010a9560c: d503221f     	esb
ffffffc010a95610: 79401660     	ldrh	w0, [x19, #0xa]
ffffffc010a95614: 79401261     	ldrh	w1, [x19, #0x8]
ffffffc010a95618: 53103800     	ubfiz	w0, w0, #16, #15
ffffffc010a9561c: 12003821     	and	w1, w1, #0x7fff
ffffffc010a95620: 2a010000     	orr	w0, w0, w1
ffffffc010a95624: 928c7e61     	mov	x1, #-0x63f4    // =-25588
ffffffc010a95628: f2a30041     	movk	x1, #0x1802, lsl #16
ffffffc010a9562c: b9000020     	str	w0, [x1]
ffffffc010a95630: d503221f     	esb
ffffffc010a95634: 79401e60     	ldrh	w0, [x19, #0xe]
ffffffc010a95638: 79401a61     	ldrh	w1, [x19, #0xc]
ffffffc010a9563c: 53103800     	ubfiz	w0, w0, #16, #15
ffffffc010a95640: 12003821     	and	w1, w1, #0x7fff
ffffffc010a95644: 2a010000     	orr	w0, w0, w1
ffffffc010a95648: 928c7de1     	mov	x1, #-0x63f0    // =-25584
ffffffc010a9564c: f2a30041     	movk	x1, #0x1802, lsl #16
ffffffc010a95650: b9000020     	str	w0, [x1]
ffffffc010a95654: d503221f     	esb
ffffffc010a95658: 79402e60     	ldrh	w0, [x19, #0x16]
ffffffc010a9565c: 79402261     	ldrh	w1, [x19, #0x10]
ffffffc010a95660: 53102000     	ubfiz	w0, w0, #16, #9
ffffffc010a95664: 12003821     	and	w1, w1, #0x7fff
ffffffc010a95668: 2a010000     	orr	w0, w0, w1
ffffffc010a9566c: 928c7d61     	mov	x1, #-0x63ec    // =-25580
ffffffc010a95670: f2a30041     	movk	x1, #0x1802, lsl #16
ffffffc010a95674: b9000020     	str	w0, [x1]
ffffffc010a95678: d503221f     	esb
ffffffc010a9567c: 79402a60     	ldrh	w0, [x19, #0x14]
ffffffc010a95680: 79402661     	ldrh	w1, [x19, #0x12]
ffffffc010a95684: 53103000     	ubfiz	w0, w0, #16, #13
ffffffc010a95688: 12003021     	and	w1, w1, #0x1fff
ffffffc010a9568c: 2a010000     	orr	w0, w0, w1
ffffffc010a95690: 928c7ce1     	mov	x1, #-0x63e8    // =-25576
ffffffc010a95694: f2a30041     	movk	x1, #0x1802, lsl #16
ffffffc010a95698: b9000020     	str	w0, [x1]
ffffffc010a9569c: d503221f     	esb
ffffffc010a956a0: d2938000     	mov	x0, #0x9c00     // =39936
ffffffc010a956a4: f2b70040     	movk	x0, #0xb802, lsl #16
ffffffc010a956a8: 97fffae2     	bl	0xffffffc010a94230 <.text+0x914230>
ffffffc010a956ac: 79403274     	ldrh	w20, [x19, #0x18]
ffffffc010a956b0: 7940367a     	ldrh	w26, [x19, #0x1a]
ffffffc010a956b4: 2a0003f6     	mov	w22, w0
ffffffc010a956b8: 79403a79     	ldrh	w25, [x19, #0x1c]
ffffffc010a956bc: 2a1503e0     	mov	w0, w21
ffffffc010a956c0: 79403e78     	ldrh	w24, [x19, #0x1e]
ffffffc010a956c4: 531c0294     	ubfiz	w20, w20, #4, #1
ffffffc010a956c8: 79404277     	ldrh	w23, [x19, #0x20]
ffffffc010a956cc: 531b035a     	ubfiz	w26, w26, #5, #1
ffffffc010a956d0: 79404673     	ldrh	w19, [x19, #0x22]
ffffffc010a956d4: 97ffff80     	bl	0xffffffc010a954d4 <.text+0x9154d4>
ffffffc010a956d8: 12000400     	and	w0, w0, #0x3
ffffffc010a956dc: 531a0339     	ubfiz	w25, w25, #6, #1
ffffffc010a956e0: 2a000281     	orr	w1, w20, w0
ffffffc010a956e4: 53190318     	ubfiz	w24, w24, #7, #1
ffffffc010a956e8: 2a1a0021     	orr	w1, w1, w26
ffffffc010a956ec: 531802f7     	ubfiz	w23, w23, #8, #1
ffffffc010a956f0: 2a190021     	orr	w1, w1, w25
ffffffc010a956f4: 53170273     	ubfiz	w19, w19, #9, #1
ffffffc010a956f8: 2a180021     	orr	w1, w1, w24
ffffffc010a956fc: 53100eb5     	ubfiz	w21, w21, #16, #4
ffffffc010a95700: 2a170021     	orr	w1, w1, w23
ffffffc010a95704: 529f8180     	mov	w0, #0xfc0c     // =64524
ffffffc010a95708: 72bffe00     	movk	w0, #0xfff0, lsl #16
ffffffc010a9570c: 2a130021     	orr	w1, w1, w19
ffffffc010a95710: 2a150021     	orr	w1, w1, w21
ffffffc010a95714: 0a0002d6     	and	w22, w22, w0
ffffffc010a95718: 2a160020     	orr	w0, w1, w22
ffffffc010a9571c: 928c7fe1     	mov	x1, #-0x6400    // =-25600
ffffffc010a95720: f2a30041     	movk	x1, #0x1802, lsl #16
ffffffc010a95724: b9000020     	str	w0, [x1]
ffffffc010a95728: d503221f     	esb
ffffffc010a9572c: a94153f3     	ldp	x19, x20, [sp, #0x10]
ffffffc010a95730: a9425bf5     	ldp	x21, x22, [sp, #0x20]
ffffffc010a95734: a94363f7     	ldp	x23, x24, [sp, #0x30]
ffffffc010a95738: a9446bf9     	ldp	x25, x26, [sp, #0x40]
ffffffc010a9573c: a8c57bfd     	ldp	x29, x30, [sp], #0x50
ffffffc010a95740: d65f03c0     	ret
ffffffc010a95744: 79448a61     	ldrh	w1, [x19, #0x244]
ffffffc010a95748: 91091273     	add	x19, x19, #0x244
ffffffc010a9574c: 17ffffa0     	b	0xffffffc010a955cc <.text+0x9155cc>
