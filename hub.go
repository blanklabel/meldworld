package main

import (
	"fmt"
	"log"
	"time"

	"github.com/blanklabel/meldworld/model"
)

type GameHub struct {
	clients     map[string]*Player
	broadcast   chan *model.ClientMessage
	register    chan *Player
	unregister  chan Player
	WorldMapped model.WorldMap
}

// Add a new unkown client
func (g *GameHub) AddNewClient(p *Player) {
	// map the player to the hub
	g.clients[p.ID] = p

	// Notify everyone of the new player
	announcement := fmt.Sprintf("Player %s joined", p.ID)
	r := &model.ClientMessage{MsgType: "client.join", Msg: announcement, Sender: "Server"}
	g.Broadcast(r)

	// TODO: Update world map with new entity

	// Give the new player the worldmap
	g.DirectMessage(g.WorldMapped, p.ID)
}

// Remove a client from the server
func (g *GameHub) RemoveClient(p Player) {
	// close the connection and delete the record
	g.clients[p.ID].conn.Close()
	delete(g.clients, p.ID)

	// Notify server of player departure
	announcement := fmt.Sprintf("Player %s left", p.ID)
	r := &model.ClientMessage{MsgType: "client.leave", Msg: announcement, Sender: "Server"}
	g.Broadcast(r)
}

// Write a message to a single client
func (g GameHub) DirectMessage(msg interface{}, userID string) {
	g.clients[userID].conn.WriteJSON(msg)
}

// Account to all clients
func (g GameHub) Broadcast(msg *model.ClientMessage) {

	// loop through all clients and give them a message
	for id, client := range g.clients {
		err := client.conn.WriteJSON(msg)
		if err != nil {
			log.Print("Didn't send")
		}
		fmt.Printf("sent msg type of '%s' contents '%s' to '%s'\n", msg.MsgType, msg.Msg, id)
	}
}

func (g GameHub) ServeGame() {

	//running at 60 FPS everyone get the ticks
	frameNS := time.Duration(int(1e9) / 60)
	clk := time.NewTicker(frameNS)

	// Run forever
	for {
		select {
		// main loop
		case <-clk.C:
			// TODO: Update world map with new entity
		case c := <-gh.register:
			gh.AddNewClient(c)
		case c := <-gh.unregister:
			gh.RemoveClient(c)
		case c := <-gh.broadcast:
			gh.Broadcast(c)
		}
	}
}
